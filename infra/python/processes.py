"""Stop owned process groups even when their original leader has already exited."""

import os
import signal
import subprocess
from collections.abc import Callable
from contextlib import suppress


def stop_group[T: (str, bytes)](
    process: subprocess.Popen[T], grace: float = 10, *, wait: Callable[[float], object] | None = None
) -> None:
    previous = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            if wait is None:
                process.wait(timeout=grace)
            else:
                wait(grace)
        except subprocess.TimeoutExpired:
            pass
        finally:
            with suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
