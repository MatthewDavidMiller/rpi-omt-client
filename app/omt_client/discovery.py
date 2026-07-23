"""Public package path for source discovery and validation."""

try:
    from ..discovery import *  # noqa: F403
except ImportError:
    from discovery import *  # type: ignore[no-redef]  # noqa: F403
