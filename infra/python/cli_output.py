"""Terminal presentation shared by Python command runners."""

import os
import sys
from collections.abc import Mapping
from contextlib import nullcontext
from typing import TextIO

from rich import box
from rich.console import Console, RenderableType
from rich.panel import Panel
from rich.status import Status
from rich.syntax import Syntax
from rich.text import Text
from rich.traceback import install

AGENT_ENVS = ('CLAUDECODE', 'CURSOR_AGENT', 'GEMINI_CLI', 'CODEX_THREAD_ID', 'OPENCODE', 'IN_CLANKER', 'in-clanker')


def is_agent(environment: Mapping[str, str]) -> bool:
    return any(environment.get(name) for name in AGENT_ENVS)


def use_color(environment: Mapping[str, str], *, supported: bool) -> bool:
    forced = any(environment.get(name, '0') not in ('', '0') for name in ('FORCE_COLOR', 'CLICOLOR_FORCE'))
    supported = (
        supported
        and environment.get('TERM') != 'dumb'
        and 'NO_COLOR' not in environment
        and 'NOCOLOR' not in environment
        and environment.get('FORCE_COLOR') != '0'
        and environment.get('CLICOLOR') != '0'
    )
    return (supported and not is_agent(environment)) or forced


def child_environment(environment: Mapping[str, str] | None = None) -> dict[str, str]:
    result = dict(os.environ if environment is None else environment)
    result.pop('NO_COLOR', None)
    result.pop('NOCOLOR', None)
    if result.get('TERM') in (None, '', 'dumb'):
        result['TERM'] = 'xterm-256color'
    result.update(FORCE_COLOR='1', CLICOLOR='1', CLICOLOR_FORCE='1', CARGO_TERM_COLOR='always', PY_COLORS='1')
    return result


def strip_ansi(value: str) -> str:
    return Text.from_ansi(value).plain


def syntax(value: str, language: str) -> Syntax:
    return Syntax(value, language, theme='ansi_dark', background_color='default', word_wrap=True)


class TerminalOutput:
    def __init__(
        self,
        *,
        stdout: TextIO | None = None,
        stderr: TextIO | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> None:
        environment = os.environ if environment is None else environment

        def console(stream: TextIO) -> Console:
            colored = use_color(environment, supported=stream.isatty())
            interactive = colored and stream.isatty() and not is_agent(environment) and not environment.get('CI')
            return Console(
                file=stream,
                force_terminal=colored,
                force_interactive=interactive,
                color_system='truecolor' if colored else None,
                no_color=not colored,
                markup=False,
                highlight=False,
            )

        self.console = console(sys.stdout if stdout is None else stdout)
        self.errors = console(sys.stderr if stderr is None else stderr)

    def install_tracebacks(self) -> None:
        install(console=self.errors, word_wrap=True, show_locals=False)

    def panel(self, content: RenderableType, *, title: str, style: str = 'blue', error: bool = False) -> None:
        console = self.errors if error else self.console
        console.print()
        console.print(
            Panel(
                content,
                title=Text(title, style='bold'),
                title_align='left',
                border_style=style,
                box=box.ROUNDED,
                padding=(1, 2),
            )
        )

    def ansi(self, value: str, *, title: str, error: bool = False) -> None:
        console = self.errors if error else self.console
        value = value.rstrip('\n')
        text = Text.from_ansi(value) if console.color_system else Text(strip_ansi(value))
        lines: list[Text] = []
        for line in text.split('\n'):
            if line.plain.strip() or (lines and lines[-1].plain.strip()):
                lines.append(line)
        if lines and not lines[-1].plain.strip():
            lines.pop()
        self.panel(Text('\n').join(lines), title=title, error=error, style='red' if error else 'blue')

    def progress(self, label: str) -> Status | nullcontext[None]:
        return self.console.status(label) if self.console.is_interactive else nullcontext()
