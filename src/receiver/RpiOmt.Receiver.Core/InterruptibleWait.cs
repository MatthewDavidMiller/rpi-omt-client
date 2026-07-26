namespace RpiOmt.Receiver.Core;

/// <summary>
/// Runs an interruptible bounded wait and gives status publication a regular
/// opportunity to emit its heartbeat.
/// </summary>
public static class InterruptibleWait
{
    public const int SliceMilliseconds = 100;

    public static void Run(
        int milliseconds,
        Func<bool> keepWaiting,
        Action? heartbeat = null,
        Action<int>? delay = null)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(milliseconds);
        ArgumentNullException.ThrowIfNull(keepWaiting);
        delay ??= Thread.Sleep;

        int remaining = milliseconds;
        while (remaining > 0 && keepWaiting())
        {
            int slice = Math.Min(remaining, SliceMilliseconds);
            delay(slice);
            remaining -= slice;
            heartbeat?.Invoke();
        }
    }
}
