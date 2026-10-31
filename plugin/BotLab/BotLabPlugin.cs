using CounterStrikeSharp.API;
using CounterStrikeSharp.API.Core;
using CounterStrikeSharp.API.Modules.Entities.Constants;
using CounterStrikeSharp.API.Modules.Utils;
using Microsoft.Extensions.Logging;

namespace BotLab;

/// Streams live game state (frames @16 Hz + discrete events) to the BotLab api over a WebSocket.
/// Display-only: the game thread just builds DTOs and enqueues them; all I/O is in LiveSender.
public sealed class BotLabPlugin : BasePlugin
{
    public const string Version = "0.1.0";
    private const int FrameEveryTicks = 4; // 64 tick / 4 = 16 Hz

    public override string ModuleName => "BotLab";
    public override string ModuleVersion => Version;
    public override string ModuleAuthor => "cs2-bot-lab";
    public override string ModuleDescription => "Streams live bot-match state to the BotLab api";

    private LiveSender? _sender;
    private string _map = "";
    private bool _mapLoaded;

    // Round bookkeeping (set from events, falls back to game rules).
    private uint _round;
    private bool _roundOver;
    private bool _matchEndSent;

    private CCSGameRulesProxy? _rulesProxy;

    /// Last known world position per player name (dead players are reported here).
    private readonly Dictionary<string, float[]> _lastPos = new();

    /// Throttle for repeated error logs from per-tick code.
    private readonly Dictionary<string, DateTime> _lastErrorLog = new();

    public override void Load(bool hotReload)
    {
        var url = Environment.GetEnvironmentVariable("BOTLAB_API_URL");
        if (string.IsNullOrWhiteSpace(url)) url = "ws://api:8080/ws/plugin";
        var serverId = Environment.GetEnvironmentVariable("BOTLAB_SERVER_ID");
        if (string.IsNullOrWhiteSpace(serverId)) serverId = SafeHostname();

        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri) || (uri.Scheme != "ws" && uri.Scheme != "wss"))
        {
            Logger.LogError("[BotLab] invalid BOTLAB_API_URL '{Url}', falling back to default", url);
            uri = new Uri("ws://api:8080/ws/plugin");
        }

        Logger.LogInformation("[BotLab] v{Version} server_id={ServerId} api={Url}", Version, serverId, uri);

        _sender = new LiveSender(uri, serverId, Version, Logger);

        RegisterListener<Listeners.OnMapStart>(OnMapStart);
        RegisterListener<Listeners.OnMapEnd>(OnMapEnd);
        RegisterListener<Listeners.OnTick>(OnTick);

        RegisterEventHandler<EventRoundStart>(OnRoundStart);
        RegisterEventHandler<EventRoundFreezeEnd>(OnFreezeEnd);
        RegisterEventHandler<EventRoundEnd>(OnRoundEnd);
        RegisterEventHandler<EventPlayerDeath>(OnPlayerDeath);
        RegisterEventHandler<EventPlayerHurt>(OnPlayerHurt);
        RegisterEventHandler<EventBombPlanted>(OnBombPlanted);
        RegisterEventHandler<EventBombDefused>(OnBombDefused);
        RegisterEventHandler<EventBombExploded>(OnBombExploded);
        RegisterEventHandler<EventHegrenadeDetonate>(OnHeDetonate);
        RegisterEventHandler<EventFlashbangDetonate>(OnFlashDetonate);
        RegisterEventHandler<EventSmokegrenadeDetonate>(OnSmokeDetonate);
        RegisterEventHandler<EventMolotovDetonate>(OnMolotovDetonate);
        RegisterEventHandler<EventDecoyStarted>(OnDecoyStarted);
        RegisterEventHandler<EventCsWinPanelMatch>(OnMatchEnd);

        // On hot reload (or late load) a map is already running.
        try
        {
            var current = Server.MapName;
            if (!string.IsNullOrEmpty(current)) BeginMap(current);
        }
        catch (Exception e)
        {
            Logger.LogWarning("[BotLab] could not read current map: {Error}", e.Message);
        }

        _sender.Start();
    }

    public override void Unload(bool hotReload)
    {
        _mapLoaded = false;
        try
        {
            _sender?.Dispose();
        }
        catch (Exception e)
        {
            Logger.LogWarning("[BotLab] unload: {Error}", e.Message);
        }
        _sender = null;
        Logger.LogInformation("[BotLab] unloaded");
    }

    // ---------------------------------------------------------------- map lifecycle

    private void OnMapStart(string mapName) => Guard("map_start", () => BeginMap(mapName));

    private void OnMapEnd() => Guard("map_end", () =>
    {
        _mapLoaded = false;
        _rulesProxy = null;
    });

    private void BeginMap(string mapName)
    {
        _map = mapName ?? "";
        _mapLoaded = !string.IsNullOrEmpty(_map);
        _rulesProxy = null;
        _round = 0;
        _roundOver = false;
        _matchEndSent = false;
        _lastPos.Clear();
        _sender?.SetMap(_map);
    }

    // ---------------------------------------------------------------- frames

    private void OnTick()
    {
        if (!_mapLoaded || _sender == null) return;
        int tick;
        try { tick = Server.TickCount; } catch { return; }
        if (tick % FrameEveryTicks != 0) return;

        try
        {
            var frame = BuildFrame(tick);
            if (frame != null) _sender.EnqueueFrame(frame);
        }
        catch (Exception e)
        {
            LogThrottled("frame", e);
        }
    }

    private FrameMessage? BuildFrame(int tick)
    {
        var rules = GetRules();
        var now = Server.CurrentTime;

        var frame = new FrameMessage { Tick = tick, Map = _map };

        string phase = Phase.Live;
        float roundTimeLeft = 0;
        uint computedRound = 1;
        if (rules != null)
        {
            computedRound = (uint)Math.Max(0, rules.TotalRoundsPlayed) + 1;
            if (rules.WarmupPeriod) phase = Phase.Warmup;
            else if (rules.GamePhase == 5 /* GAMEPHASE_MATCH_ENDED */ || _roundOver) phase = Phase.Over;
            else if (rules.FreezePeriod) phase = Phase.Freezetime;

            float roundTime = Math.Max(0, rules.RoundTime);
            roundTimeLeft = Math.Clamp(rules.RoundStartTime + roundTime - now, 0, roundTime);
        }
        else if (_roundOver)
        {
            phase = Phase.Over;
        }

        frame.Phase = phase;
        frame.Round = _round > 0 && phase != Phase.Warmup ? _round : computedRound;
        frame.RoundTimeLeft = San(roundTimeLeft);

        var (ct, t) = GetScores();
        frame.ScoreCt = ct;
        frame.ScoreT = t;
        frame.Bomb = BuildBomb(now);

        foreach (var c in Utilities.GetPlayers())
        {
            try
            {
                var ps = BuildPlayer(c, now);
                if (ps != null) frame.Players.Add(ps);
            }
            catch (Exception e)
            {
                LogThrottled("player", e);
            }
        }

        return frame;
    }

    private PlayerState? BuildPlayer(CCSPlayerController? c, float now)
    {
        if (c == null || !c.IsValid || c.IsHLTV) return null;
        var side = Side.FromTeamNum(c.TeamNum);
        if (side == null) return null;

        var name = c.PlayerName ?? "";
        var ps = new PlayerState { Name = name, Side = side };

        ps.Money = c.InGameMoneyServices?.Account ?? 0;

        var pawn = c.PlayerPawn.Value;
        bool pawnOk = pawn != null && pawn.IsValid;
        ps.Alive = c.PawnIsAlive && pawnOk && pawn!.Health > 0;

        if (!pawnOk)
        {
            ps.Alive = false;
            ps.Pos = LastPos(name);
            return ps;
        }

        if (ps.Alive)
        {
            ps.Hp = Math.Max(0, pawn!.Health);
            ps.Armor = Math.Max(0, pawn.ArmorValue);
            var pos = Vec(pawn.AbsOrigin);
            if (pos != null)
            {
                ps.Pos = pos;
                _lastPos[name] = pos;
            }
            else
            {
                ps.Pos = LastPos(name);
            }

            var eye = pawn.EyeAngles;
            if (eye != null)
            {
                ps.Pitch = San(eye.X);
                ps.Yaw = San(eye.Y);
            }

            ps.EquipValue = pawn.CurrentEquipmentValue;

            var items = pawn.ItemServices == null
                ? null
                : new CCSPlayer_ItemServices(pawn.ItemServices.Handle);
            if (items != null)
            {
                ps.Helmet = items.HasHelmet;
                ps.Defuser = items.HasDefuser;
            }

            ps.Flash = San(FlashRemaining(pawn, now));

            var ws = pawn.WeaponServices;
            if (ws != null)
            {
                var active = ws.ActiveWeapon.Value;
                if (active != null && active.IsValid) ps.ActiveWeapon = WeaponName(active);

                var my = ws.MyWeapons;
                for (int i = 0; i < my.Count; i++)
                {
                    var w = my[i].Value;
                    if (w != null && w.IsValid) ps.Weapons.Add(WeaponName(w));
                }
            }
        }
        else
        {
            // Dead: report at the last known position (where they died).
            ps.Pos = LastPos(name);
        }

        return ps;
    }

    private static float FlashRemaining(CCSPlayerPawn pawn, float now)
    {
        float fromBlindUntil = pawn.BlindUntilTime - now;
        float fromDuration = pawn.FlashDuration > 0 ? pawn.BlindStartTime + pawn.FlashDuration - now : 0;
        float left = Math.Max(fromBlindUntil, fromDuration);
        if (!float.IsFinite(left) || left <= 0) return 0;
        // Guard against stale timestamps (e.g. after map change); a flash never lasts > ~6 s.
        return Math.Min(left, 10f);
    }

    private BombState BuildBomb(float now)
    {
        var bomb = new BombState();

        foreach (var c4 in Utilities.FindAllEntitiesByDesignerName<CPlantedC4>("planted_c4"))
        {
            if (c4 == null || !c4.IsValid) continue;
            if (c4.HasExploded || c4.BombDefused || !c4.BombTicking) continue;
            bomb.Planted = true;
            bomb.Site = SiteName(c4.BombSite);
            bomb.TimeLeft = San(Math.Max(0, c4.C4Blow - now));
            bomb.Defusing = c4.BeingDefused;
            bomb.Pos = Vec(c4.AbsOrigin);
            return bomb;
        }

        foreach (var c4 in Utilities.FindAllEntitiesByDesignerName<CC4>("weapon_c4"))
        {
            if (c4 == null || !c4.IsValid) continue;
            float[]? pos = null;
            var owner = c4.OwnerEntity.Value;
            if (owner != null && owner.IsValid) pos = Vec(owner.AbsOrigin);
            pos ??= Vec(c4.AbsOrigin);
            bomb.Pos = pos;
            return bomb;
        }

        return bomb;
    }

    // ---------------------------------------------------------------- events

    private HookResult OnRoundStart(EventRoundStart ev, GameEventInfo info) => GuardEvent("round_start", () =>
    {
        var rules = GetRules();
        _round = rules != null ? (uint)Math.Max(0, rules.TotalRoundsPlayed) + 1 : _round + 1;
        _roundOver = false;
        Emit(new RoundStartEvent { Round = _round });
    });

    private HookResult OnFreezeEnd(EventRoundFreezeEnd ev, GameEventInfo info) => GuardEvent("freeze_end", () =>
    {
        Emit(new FreezeEndEvent { Round = CurrentRound() });
    });

    private HookResult OnRoundEnd(EventRoundEnd ev, GameEventInfo info) => GuardEvent("round_end", () =>
    {
        _roundOver = true;
        var winner = Side.FromTeamNum(ev.Winner);
        if (winner == null)
        {
            // Draws / warmup restarts have no CT/T winner; the wire type requires one, so skip.
            Logger.LogDebug("[BotLab] round_end without winner (team {Winner}), not sent", ev.Winner);
            return;
        }
        Emit(new RoundEndEvent { Round = CurrentRound(), Winner = winner, Reason = ReasonName(ev.Reason) });
    });

    private HookResult OnPlayerDeath(EventPlayerDeath ev, GameEventInfo info) => GuardEvent("kill", () =>
    {
        var victim = ev.Userid;
        if (victim == null || !victim.IsValid) return;
        var attacker = ev.Attacker;
        bool hasAttacker = attacker != null && attacker.IsValid && attacker.Index != victim.Index;
        var assister = ev.Assister;
        bool hasAssister = assister != null && assister.IsValid;

        var victimName = victim.PlayerName ?? "";
        var victimPos = PawnPos(victim) ?? LastPos(victimName);
        _lastPos[victimName] = victimPos;

        Emit(new KillEvent
        {
            Attacker = hasAttacker ? attacker!.PlayerName : null,
            Victim = victimName,
            Assister = hasAssister ? assister!.PlayerName : null,
            Weapon = EventWeapon(ev.Weapon),
            Headshot = ev.Headshot,
            AttackerPos = hasAttacker ? PawnPos(attacker!) : null,
            VictimPos = victimPos,
        });
    });

    private HookResult OnPlayerHurt(EventPlayerHurt ev, GameEventInfo info) => GuardEvent("hurt", () =>
    {
        var victim = ev.Userid;
        if (victim == null || !victim.IsValid) return;
        var attacker = ev.Attacker;
        bool hasAttacker = attacker != null && attacker.IsValid;
        Emit(new HurtEvent
        {
            Attacker = hasAttacker ? attacker!.PlayerName : null,
            Victim = victim.PlayerName ?? "",
            Weapon = EventWeapon(ev.Weapon),
            Damage = ev.DmgHealth,
            ArmorDamage = ev.DmgArmor,
            Hitgroup = ev.Hitgroup,
        });
    });

    private HookResult OnBombPlanted(EventBombPlanted ev, GameEventInfo info) => GuardEvent("bomb_planted", () =>
    {
        Emit(new BombPlantedEvent { Player = NameOf(ev.Userid), Site = EventSite(ev.Site) });
    });

    private HookResult OnBombDefused(EventBombDefused ev, GameEventInfo info) => GuardEvent("bomb_defused", () =>
    {
        Emit(new BombDefusedEvent { Player = NameOf(ev.Userid), Site = EventSite(ev.Site) });
    });

    private HookResult OnBombExploded(EventBombExploded ev, GameEventInfo info) => GuardEvent("bomb_exploded", () =>
    {
        Emit(new BombExplodedEvent { Site = EventSite(ev.Site) });
    });

    private HookResult OnHeDetonate(EventHegrenadeDetonate ev, GameEventInfo info) =>
        GuardEvent("he", () => EmitNade(ev.Userid, GrenadeKind.He, ev.X, ev.Y, ev.Z));

    private HookResult OnFlashDetonate(EventFlashbangDetonate ev, GameEventInfo info) =>
        GuardEvent("flash", () => EmitNade(ev.Userid, GrenadeKind.Flash, ev.X, ev.Y, ev.Z));

    private HookResult OnSmokeDetonate(EventSmokegrenadeDetonate ev, GameEventInfo info) =>
        GuardEvent("smoke", () => EmitNade(ev.Userid, GrenadeKind.Smoke, ev.X, ev.Y, ev.Z));

    private HookResult OnMolotovDetonate(EventMolotovDetonate ev, GameEventInfo info) =>
        GuardEvent("molotov", () => EmitNade(ev.Userid, GrenadeKind.Molotov, ev.X, ev.Y, ev.Z));

    // decoy_started = the moment the decoy lands and starts "firing" (decoy_detonate is when it expires).
    private HookResult OnDecoyStarted(EventDecoyStarted ev, GameEventInfo info) =>
        GuardEvent("decoy", () => EmitNade(ev.Userid, GrenadeKind.Decoy, ev.X, ev.Y, ev.Z));

    private HookResult OnMatchEnd(EventCsWinPanelMatch ev, GameEventInfo info) => GuardEvent("match_end", () =>
    {
        if (_matchEndSent) return;
        _matchEndSent = true;
        _roundOver = true;
        var (ct, t) = GetScores();
        Emit(new MatchEndEvent { ScoreCt = ct, ScoreT = t });
    });

    private void EmitNade(CCSPlayerController? thrower, string kind, float x, float y, float z)
    {
        Emit(new GrenadeDetonateEvent
        {
            Thrower = thrower != null && thrower.IsValid ? thrower.PlayerName : null,
            Grenade = kind,
            Pos = new[] { San(x), San(y), San(z) },
        });
    }

    private void Emit(EventMessage ev)
    {
        if (_sender == null) return;
        ev.Tick = Server.TickCount;
        _sender.EnqueueEvent(ev);
    }

    // ---------------------------------------------------------------- helpers

    private uint CurrentRound()
    {
        if (_round > 0) return _round;
        var rules = GetRules();
        return rules != null ? (uint)Math.Max(0, rules.TotalRoundsPlayed) + 1 : 1;
    }

    private CCSGameRules? GetRules()
    {
        if (_rulesProxy == null || !_rulesProxy.IsValid)
        {
            _rulesProxy = Utilities.FindAllEntitiesByDesignerName<CCSGameRulesProxy>("cs_gamerules")
                .FirstOrDefault(p => p != null && p.IsValid);
        }
        return _rulesProxy?.GameRules;
    }

    private static (uint ct, uint t) GetScores()
    {
        uint ct = 0, t = 0;
        foreach (var team in Utilities.FindAllEntitiesByDesignerName<CCSTeam>("cs_team_manager"))
        {
            if (team == null || !team.IsValid) continue;
            var score = (uint)Math.Max(0, team.Score);
            if (team.TeamNum == 3) ct = score;
            else if (team.TeamNum == 2) t = score;
        }
        return (ct, t);
    }

    private static string? SiteName(int bombSite) => bombSite switch { 0 => "A", 1 => "B", _ => null };

    /// bomb_* events carry the entity index of the func_bomb_target trigger.
    private string? EventSite(int siteIndex)
    {
        try
        {
            if (siteIndex > 0)
            {
                var target = Utilities.GetEntityFromIndex<CBombTarget>(siteIndex);
                if (target != null && target.IsValid && target.DesignerName == "func_bomb_target")
                    return target.IsBombSiteB ? "B" : "A";
            }
        }
        catch { /* fall through */ }

        foreach (var c4 in Utilities.FindAllEntitiesByDesignerName<CPlantedC4>("planted_c4"))
        {
            if (c4 != null && c4.IsValid) return SiteName(c4.BombSite);
        }
        return null;
    }

    /// round_end `reason` (CounterStrikeSharp RoundEndReason numbering) -> stable snake_case string.
    private static string ReasonName(int reason) => (RoundEndReason)reason switch
    {
        RoundEndReason.TargetBombed => "target_bombed",
        RoundEndReason.BombDefused => "bomb_defused",
        RoundEndReason.CTsWin => "t_eliminated",
        RoundEndReason.TerroristsWin => "ct_eliminated",
        RoundEndReason.TargetSaved => "time_ran_out",
        RoundEndReason.RoundDraw => "draw",
        RoundEndReason.TerroristsSurrender => "t_surrender",
        RoundEndReason.CTsSurrender => "ct_surrender",
        RoundEndReason.GameCommencing => "game_commencing",
        RoundEndReason.AllHostageRescued => "hostages_rescued",
        RoundEndReason.HostagesNotRescued => "hostages_not_rescued",
        _ => $"reason_{reason}",
    };

    private static string NameOf(CCSPlayerController? c) =>
        c != null && c.IsValid ? c.PlayerName ?? "" : "";

    /// Event weapon names come without the prefix ("ak47", "hegrenade", "world", "inferno").
    private static string EventWeapon(string? w)
    {
        if (string.IsNullOrEmpty(w)) return "";
        if (w.StartsWith("weapon_") || w == "world" || w == "inferno" || w == "planted_c4" || w.Contains("projectile"))
            return w;
        return "weapon_" + w;
    }

    /// Designer name, corrected via item definition index for variants that share an entity class
    /// (e.g. USP-S is a weapon_hkp2000 entity, M4A1-S a weapon_m4a1).
    private static string WeaponName(CBasePlayerWeapon w)
    {
        try
        {
            ushort def = w.AttributeManager.Item.ItemDefinitionIndex;
            switch (def)
            {
                case 23: return "weapon_mp5sd";
                case 60: return "weapon_m4a1_silencer";
                case 61: return "weapon_usp_silencer";
                case 63: return "weapon_cz75a";
                case 64: return "weapon_revolver";
            }
        }
        catch { /* fall back to designer name */ }
        return w.DesignerName ?? "";
    }

    private static float[]? PawnPos(CCSPlayerController c)
    {
        var pawn = c.PlayerPawn.Value;
        if (pawn == null || !pawn.IsValid) return null;
        return Vec(pawn.AbsOrigin);
    }

    private float[] LastPos(string name) =>
        _lastPos.TryGetValue(name, out var p) ? p : new float[3];

    private static float[]? Vec(Vector? v)
    {
        if (v == null) return null;
        return new[] { San(v.X), San(v.Y), San(v.Z) };
    }

    private static float San(float f) => float.IsFinite(f) ? f : 0f;

    private static string SafeHostname()
    {
        try { return Environment.MachineName; } catch { return "unknown"; }
    }

    private void Guard(string what, Action a)
    {
        try { a(); }
        catch (Exception e) { LogThrottled(what, e); }
    }

    private HookResult GuardEvent(string what, Action a)
    {
        if (_mapLoaded) Guard(what, a);
        return HookResult.Continue;
    }

    private void LogThrottled(string key, Exception e)
    {
        var now = DateTime.UtcNow;
        if (_lastErrorLog.TryGetValue(key, out var last) && now - last < TimeSpan.FromSeconds(10)) return;
        _lastErrorLog[key] = now;
        Logger.LogError(e, "[BotLab] error in {Handler}", key);
    }
}
