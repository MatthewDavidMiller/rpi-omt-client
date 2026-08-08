"""Request-correlated diagnostics and bounded support archives.

Split by trust boundary rather than by type:

- `checks.py` runs what the container can answer itself;
- `host.py` owns the correlated request channel to the privileged collector;
- `bundle.py` composes both and lays out the archive.

`RuntimeDiagnostics` is the only name outside this package uses; it satisfies
`services.protocols.DiagnosticsService`.
"""

from .bundle import RuntimeDiagnostics

__all__ = ("RuntimeDiagnostics",)
