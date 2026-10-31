// Wire DTOs for the plugin -> api WebSocket.
//
// These MIRROR crates/lab-core/src/live.rs (`PluginMessage`, `LiveFrame`, `PlayerState`,
// `BombState`, `GameEvent`, `EventKind`), which is the source of truth. Keep them in sync:
//   * every message carries `"type"` ("hello" | "frame" | "event"), frame fields are inline;
//   * events carry `"tick"` and `"kind"` (snake_case EventKind variant) with variant fields inline;
//   * field names are snake_case, Option<T> -> JSON null (never omitted), [f32;3] -> [x,y,z];
//   * Side is "CT" / "T", BombSite is "A" / "B", RoundPhase is warmup|freezetime|live|over,
//     Grenade is he|flash|smoke|molotov|decoy.
// Serialize with `JsonSerializer.Serialize(msg, msg.GetType(), ...)` so derived properties are written.

using System.Text.Json.Serialization;

namespace BotLab;

public static class Side
{
    public const string CT = "CT";
    public const string T = "T";

    /// CS team number: 2 = T, 3 = CT.
    public static string? FromTeamNum(int n) => n switch { 2 => T, 3 => CT, _ => null };
}

public static class Phase
{
    public const string Warmup = "warmup";
    public const string Freezetime = "freezetime";
    public const string Live = "live";
    public const string Over = "over";
}

public static class GrenadeKind
{
    public const string He = "he";
    public const string Flash = "flash";
    public const string Smoke = "smoke";
    public const string Molotov = "molotov";
    public const string Decoy = "decoy";
}

/// Base of everything sent to the api (`PluginMessage`).
public abstract class PluginMessage
{
    protected PluginMessage(string type) => Type = type;

    [JsonPropertyName("type")]
    [JsonPropertyOrder(-100)]
    public string Type { get; }
}

public sealed class HelloMessage : PluginMessage
{
    public HelloMessage() : base("hello") { }

    [JsonPropertyName("server_id")] public string ServerId { get; set; } = "";
    [JsonPropertyName("map")] public string Map { get; set; } = "";
    [JsonPropertyName("plugin_version")] public string PluginVersion { get; set; } = "";
}

public sealed class BombState
{
    [JsonPropertyName("planted")] public bool Planted { get; set; }
    /// "A" / "B" / null.
    [JsonPropertyName("site")] public string? Site { get; set; }
    /// Seconds until detonation while planted.
    [JsonPropertyName("time_left")] public float? TimeLeft { get; set; }
    [JsonPropertyName("defusing")] public bool Defusing { get; set; }
    /// Planted, dropped or carried position.
    [JsonPropertyName("pos")] public float[]? Pos { get; set; }
}

public sealed class PlayerState
{
    [JsonPropertyName("name")] public string Name { get; set; } = "";
    [JsonPropertyName("side")] public string Side { get; set; } = BotLab.Side.T;
    [JsonPropertyName("alive")] public bool Alive { get; set; }
    [JsonPropertyName("hp")] public int Hp { get; set; }
    [JsonPropertyName("armor")] public int Armor { get; set; }
    [JsonPropertyName("helmet")] public bool Helmet { get; set; }
    [JsonPropertyName("defuser")] public bool Defuser { get; set; }
    [JsonPropertyName("money")] public int Money { get; set; }
    [JsonPropertyName("equip_value")] public int EquipValue { get; set; }
    [JsonPropertyName("pos")] public float[] Pos { get; set; } = new float[3];
    [JsonPropertyName("yaw")] public float Yaw { get; set; }
    [JsonPropertyName("pitch")] public float Pitch { get; set; }
    [JsonPropertyName("active_weapon")] public string ActiveWeapon { get; set; } = "";
    [JsonPropertyName("weapons")] public List<string> Weapons { get; set; } = new();
    /// Remaining flash blindness, seconds.
    [JsonPropertyName("flash")] public float Flash { get; set; }
}

/// `PluginMessage::Frame(LiveFrame)` — LiveFrame fields inline next to "type".
public sealed class FrameMessage : PluginMessage
{
    public FrameMessage() : base("frame") { }

    [JsonPropertyName("tick")] public int Tick { get; set; }
    [JsonPropertyName("map")] public string Map { get; set; } = "";
    /// 1-based round number.
    [JsonPropertyName("round")] public uint Round { get; set; }
    [JsonPropertyName("phase")] public string Phase { get; set; } = BotLab.Phase.Warmup;
    /// Seconds left on the round clock (excluding the bomb timer).
    [JsonPropertyName("round_time_left")] public float RoundTimeLeft { get; set; }
    [JsonPropertyName("score_ct")] public uint ScoreCt { get; set; }
    [JsonPropertyName("score_t")] public uint ScoreT { get; set; }
    [JsonPropertyName("bomb")] public BombState Bomb { get; set; } = new();
    [JsonPropertyName("players")] public List<PlayerState> Players { get; set; } = new();
}

/// `PluginMessage::Event(GameEvent)` — `tick`, `kind` and the variant's fields inline.
public abstract class EventMessage : PluginMessage
{
    protected EventMessage(string kind) : base("event") => Kind = kind;

    [JsonPropertyName("tick")]
    [JsonPropertyOrder(-99)]
    public int Tick { get; set; }

    [JsonPropertyName("kind")]
    [JsonPropertyOrder(-98)]
    public string Kind { get; }
}

public sealed class RoundStartEvent : EventMessage
{
    public RoundStartEvent() : base("round_start") { }
    [JsonPropertyName("round")] public uint Round { get; set; }
}

public sealed class FreezeEndEvent : EventMessage
{
    public FreezeEndEvent() : base("freeze_end") { }
    [JsonPropertyName("round")] public uint Round { get; set; }
}

public sealed class RoundEndEvent : EventMessage
{
    public RoundEndEvent() : base("round_end") { }
    [JsonPropertyName("round")] public uint Round { get; set; }
    /// "CT" / "T".
    [JsonPropertyName("winner")] public string Winner { get; set; } = BotLab.Side.CT;
    [JsonPropertyName("reason")] public string Reason { get; set; } = "";
}

public sealed class KillEvent : EventMessage
{
    public KillEvent() : base("kill") { }
    [JsonPropertyName("attacker")] public string? Attacker { get; set; }
    [JsonPropertyName("victim")] public string Victim { get; set; } = "";
    [JsonPropertyName("assister")] public string? Assister { get; set; }
    [JsonPropertyName("weapon")] public string Weapon { get; set; } = "";
    [JsonPropertyName("headshot")] public bool Headshot { get; set; }
    [JsonPropertyName("attacker_pos")] public float[]? AttackerPos { get; set; }
    [JsonPropertyName("victim_pos")] public float[] VictimPos { get; set; } = new float[3];
}

public sealed class HurtEvent : EventMessage
{
    public HurtEvent() : base("hurt") { }
    [JsonPropertyName("attacker")] public string? Attacker { get; set; }
    [JsonPropertyName("victim")] public string Victim { get; set; } = "";
    [JsonPropertyName("weapon")] public string Weapon { get; set; } = "";
    [JsonPropertyName("damage")] public int Damage { get; set; }
    [JsonPropertyName("armor_damage")] public int ArmorDamage { get; set; }
    [JsonPropertyName("hitgroup")] public int Hitgroup { get; set; }
}

public sealed class BombPlantedEvent : EventMessage
{
    public BombPlantedEvent() : base("bomb_planted") { }
    [JsonPropertyName("player")] public string Player { get; set; } = "";
    [JsonPropertyName("site")] public string? Site { get; set; }
}

public sealed class BombDefusedEvent : EventMessage
{
    public BombDefusedEvent() : base("bomb_defused") { }
    [JsonPropertyName("player")] public string Player { get; set; } = "";
    [JsonPropertyName("site")] public string? Site { get; set; }
}

public sealed class BombExplodedEvent : EventMessage
{
    public BombExplodedEvent() : base("bomb_exploded") { }
    [JsonPropertyName("site")] public string? Site { get; set; }
}

public sealed class GrenadeDetonateEvent : EventMessage
{
    public GrenadeDetonateEvent() : base("grenade_detonate") { }
    [JsonPropertyName("thrower")] public string? Thrower { get; set; }
    /// he | flash | smoke | molotov | decoy.
    [JsonPropertyName("grenade")] public string Grenade { get; set; } = GrenadeKind.He;
    [JsonPropertyName("pos")] public float[] Pos { get; set; } = new float[3];
}

public sealed class MatchEndEvent : EventMessage
{
    public MatchEndEvent() : base("match_end") { }
    [JsonPropertyName("score_ct")] public uint ScoreCt { get; set; }
    [JsonPropertyName("score_t")] public uint ScoreT { get; set; }
}
