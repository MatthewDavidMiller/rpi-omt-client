"""Public package path for OMT SDK network-document transforms."""

try:
    from ..network_config import *  # noqa: F403
except ImportError:
    from network_config import *  # type: ignore[no-redef]  # noqa: F403
