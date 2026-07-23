namespace RpiOmt.Deployer.Core;

public sealed class ActionController
{
    public OperationState State { get; private set; } = OperationState.Idle;

    public string Stage { get; private set; } = "idle";

    public bool Cancellable { get; private set; } = true;

    public bool IsActive => State is OperationState.Running or OperationState.Cancelling;

    public bool Start()
    {
        if (IsActive)
        {
            return false;
        }

        State = OperationState.Running;
        Stage = "starting";
        Cancellable = true;
        return true;
    }

    public void Progress(ProgressEventArgs progress)
    {
        if (State != OperationState.Cancelling)
        {
            State = OperationState.Running;
        }

        if (!string.IsNullOrEmpty(progress.Stage))
        {
            Stage = progress.Stage;
        }

        Cancellable = progress.Cancellable;
    }

    public bool RequestCancellation()
    {
        if (!IsActive || !Cancellable)
        {
            return false;
        }

        State = OperationState.Cancelling;
        return true;
    }

    public void Finish(OperationState state)
    {
        if (state is not (OperationState.Succeeded or OperationState.Failed or OperationState.Cancelled))
        {
            throw new ArgumentOutOfRangeException(nameof(state), "Operation finish state must be terminal.");
        }

        State = state;
        Stage = "complete";
        Cancellable = false;
    }
}
