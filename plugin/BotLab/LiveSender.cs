using System.Net.WebSockets;
using System.Text.Json;
using System.Threading.Channels;
using Microsoft.Extensions.Logging;

namespace BotLab;

/// Owns the WebSocket to the api. The game thread only calls the non-blocking Enqueue* methods;
/// everything else (serialization, connect, send, reconnect) runs on a background task.
public sealed class LiveSender : IDisposable
{
    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        // NaN/Infinity would throw here; the game thread sanitizes all floats before enqueueing.
        WriteIndented = false,
    };

    private readonly Uri _url;
    private readonly string _serverId;
    private readonly string _pluginVersion;
    private readonly ILogger _log;

    // Frames are disposable (16 Hz, only the newest matters): small buffer, drop oldest.
    private readonly Channel<PluginMessage> _frames = Channel.CreateBounded<PluginMessage>(
        new BoundedChannelOptions(8) { FullMode = BoundedChannelFullMode.DropOldest, SingleReader = true });

    // Events + hello: large buffer so they survive short disconnects; only drop on overflow.
    private readonly Channel<PluginMessage> _events = Channel.CreateBounded<PluginMessage>(
        new BoundedChannelOptions(4096) { FullMode = BoundedChannelFullMode.DropOldest, SingleReader = true });

    private readonly CancellationTokenSource _cts = new();
    private Task? _loop;
    private volatile string _map = "";

    public LiveSender(Uri url, string serverId, string pluginVersion, ILogger log)
    {
        _url = url;
        _serverId = serverId;
        _pluginVersion = pluginVersion;
        _log = log;
    }

    public void Start() => _loop = Task.Run(() => RunAsync(_cts.Token));

    /// Called on map change (game thread). The hello is queued in order with events.
    public void SetMap(string map)
    {
        _map = map ?? "";
        _events.Writer.TryWrite(MakeHello());
    }

    public void EnqueueFrame(FrameMessage frame) => _frames.Writer.TryWrite(frame);

    public void EnqueueEvent(EventMessage ev) => _events.Writer.TryWrite(ev);

    private HelloMessage MakeHello() => new() { ServerId = _serverId, Map = _map, PluginVersion = _pluginVersion };

    private async Task RunAsync(CancellationToken ct)
    {
        var backoff = TimeSpan.FromSeconds(1);
        while (!ct.IsCancellationRequested)
        {
            using var ws = new ClientWebSocket();
            ws.Options.KeepAliveInterval = TimeSpan.FromSeconds(15);
            try
            {
                using (var connectCts = CancellationTokenSource.CreateLinkedTokenSource(ct))
                {
                    connectCts.CancelAfter(TimeSpan.FromSeconds(10));
                    await ws.ConnectAsync(_url, connectCts.Token).ConfigureAwait(false);
                }
                _log.LogInformation("[BotLab] connected to {Url}", _url);
                backoff = TimeSpan.FromSeconds(1);

                // Stale frames from while we were offline are useless.
                while (_frames.Reader.TryRead(out _)) { }

                await SendAsync(ws, MakeHello(), ct).ConfigureAwait(false);
                await PumpAsync(ws, ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                break;
            }
            catch (Exception e)
            {
                _log.LogWarning("[BotLab] websocket {Url}: {Error}; retrying in {Delay}s",
                    _url, e.Message, backoff.TotalSeconds);
            }

            if (ws.State == WebSocketState.Open)
            {
                try
                {
                    using var closeCts = new CancellationTokenSource(TimeSpan.FromSeconds(1));
                    await ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "bye", closeCts.Token)
                        .ConfigureAwait(false);
                }
                catch { /* best effort */ }
            }

            if (ct.IsCancellationRequested) break;
            try { await Task.Delay(backoff, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { break; }
            backoff = TimeSpan.FromSeconds(Math.Min(backoff.TotalSeconds * 2, 30));
        }
    }

    /// Sends queued messages until the socket dies or we are cancelled.
    /// Events/hello first (in order), then only the newest pending frame.
    private async Task PumpAsync(ClientWebSocket ws, CancellationToken ct)
    {
        using var connCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        // Drain incoming data so close frames / pings are processed and we notice a dead peer.
        var receive = ReceiveLoopAsync(ws, connCts);

        try
        {
            while (!connCts.IsCancellationRequested && ws.State == WebSocketState.Open)
            {
                var sent = false;
                while (_events.Reader.TryRead(out var ev))
                {
                    await SendAsync(ws, ev, connCts.Token).ConfigureAwait(false);
                    sent = true;
                }

                PluginMessage? latest = null;
                while (_frames.Reader.TryRead(out var f)) latest = f;
                if (latest != null)
                {
                    await SendAsync(ws, latest, connCts.Token).ConfigureAwait(false);
                    sent = true;
                }

                if (sent) continue;

                var waitEvents = _events.Reader.WaitToReadAsync(connCts.Token).AsTask();
                var waitFrames = _frames.Reader.WaitToReadAsync(connCts.Token).AsTask();
                var done = await Task.WhenAny(waitEvents, waitFrames, receive).ConfigureAwait(false);
                if (done == receive) break;
                if (done.IsCanceled) break;
                if (done is Task<bool> { IsCompletedSuccessfully: true, Result: false }) return; // channels completed (shutdown)
            }
        }
        finally
        {
            connCts.Cancel();
            try { await receive.ConfigureAwait(false); } catch { /* ignored */ }
        }

        ct.ThrowIfCancellationRequested();
        throw new WebSocketException($"connection lost (state {ws.State})");
    }

    private static async Task ReceiveLoopAsync(ClientWebSocket ws, CancellationTokenSource connCts)
    {
        var buf = new byte[4096];
        try
        {
            while (ws.State == WebSocketState.Open && !connCts.IsCancellationRequested)
            {
                var r = await ws.ReceiveAsync(buf, connCts.Token).ConfigureAwait(false);
                if (r.MessageType == WebSocketMessageType.Close) break;
            }
        }
        catch { /* socket error or cancellation: handled by caller */ }
    }

    private async Task SendAsync(ClientWebSocket ws, PluginMessage msg, CancellationToken ct)
    {
        byte[] bytes;
        try
        {
            bytes = JsonSerializer.SerializeToUtf8Bytes(msg, msg.GetType(), JsonOpts);
        }
        catch (Exception e)
        {
            _log.LogError("[BotLab] failed to serialize {Type}: {Error}", msg.GetType().Name, e.Message);
            return;
        }

        using var sendCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        sendCts.CancelAfter(TimeSpan.FromSeconds(5));
        await ws.SendAsync(bytes, WebSocketMessageType.Text, true, sendCts.Token).ConfigureAwait(false);
    }

    /// Stops the background task. Waits at most `timeout` so Unload never stalls the server.
    public void Stop(TimeSpan timeout)
    {
        try
        {
            _frames.Writer.TryComplete();
            _events.Writer.TryComplete();
            _cts.Cancel();
            _loop?.Wait(timeout);
        }
        catch (Exception e)
        {
            _log.LogWarning("[BotLab] sender stop: {Error}", e.Message);
        }
    }

    public void Dispose()
    {
        Stop(TimeSpan.FromSeconds(1));
        _cts.Dispose();
    }

    /// Serialized JSON of a message (same options as the wire). Used by the debug command.
    public static string ToJson(PluginMessage m) => JsonSerializer.Serialize(m, m.GetType(), JsonOpts);
}
