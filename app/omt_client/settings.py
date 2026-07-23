"""Public package path for application settings."""

try:
    from ..settings import *  # noqa: F403
except ImportError:
    from settings import *  # type: ignore[no-redef]  # noqa: F403
