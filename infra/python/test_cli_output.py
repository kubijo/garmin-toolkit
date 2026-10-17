import unittest
from io import StringIO

from cli_output import AGENT_ENVS, TerminalOutput, child_environment, strip_ansi, syntax, use_color


class TerminalBuffer(StringIO):
    def isatty(self) -> bool:
        return True


class OutputTests(unittest.TestCase):
    def test_color_condition(self) -> None:
        for supported in (False, True):
            for agent in (False, True):
                for forced in (False, True):
                    with self.subTest(supported=supported, agent=agent, forced=forced):
                        environment = {'TERM': 'xterm-256color'}
                        if agent:
                            environment['IN_CLANKER'] = '1'
                        if forced:
                            environment['FORCE_COLOR'] = '1'
                        self.assertEqual(
                            use_color(environment, supported=supported), (supported and not agent) or forced
                        )
        for name in AGENT_ENVS:
            self.assertFalse(use_color({name: '1'}, supported=True), name)

    def test_explicit_force_overrides_plain_output_preferences(self) -> None:
        for force in ('FORCE_COLOR', 'CLICOLOR_FORCE'):
            environment = {'NO_COLOR': '1', 'CLICOLOR': '0', 'TERM': 'dumb', 'IN_CLANKER': '1', force: '1'}
            self.assertTrue(use_color(environment, supported=False))
        for environment in ({'NO_COLOR': ''}, {'FORCE_COLOR': '0'}, {'CLICOLOR': '0'}, {'TERM': 'dumb'}):
            self.assertFalse(use_color(environment, supported=True))

    def test_children_receive_color_overrides_without_mutating_parent(self) -> None:
        environment = {'NO_COLOR': '1', 'NOCOLOR': '1', 'TERM': 'dumb', 'FORCE_COLOR': '0', 'CARGO_TERM_COLOR': 'never'}
        child = child_environment(environment)
        self.assertNotIn('NO_COLOR', child)
        self.assertNotIn('NOCOLOR', child)
        self.assertEqual(child['FORCE_COLOR'], '1')
        self.assertEqual(child['CARGO_TERM_COLOR'], 'always')
        self.assertEqual(child['TERM'], 'xterm-256color')
        self.assertEqual(environment['FORCE_COLOR'], '0')
        self.assertEqual(environment['NO_COLOR'], '1')

    def test_child_styles_are_preserved_or_stripped_in_panels(self) -> None:
        for force in (False, True):
            with self.subTest(force=force):
                stream = TerminalBuffer()
                environment = {'IN_CLANKER': '1'}
                if force:
                    environment['FORCE_COLOR'] = '1'
                output = TerminalOutput(stdout=stream, stderr=stream, environment=environment)
                output.ansi('\x1b[31mred [literal]\x1b[0m', title='Subprocess output')
                rendered = stream.getvalue()
                self.assertEqual('\x1b[' in rendered, force)
                if force:
                    self.assertIn('\x1b[31m', rendered)
                self.assertIn('red [literal]', strip_ansi(rendered))
                self.assertIn('╭', rendered)
                self.assertFalse(output.console.is_interactive)

    def test_structured_text_uses_a_lexer_without_leaking_ansi_to_plain_output(self) -> None:
        source = '{"count": 1}'
        highlighted = syntax(source, 'json')
        assert highlighted.lexer is not None
        self.assertEqual(highlighted.lexer.name, 'JSON')
        for force in (False, True):
            stream = StringIO()
            output = TerminalOutput(stdout=stream, environment={'FORCE_COLOR': '1'} if force else {})
            output.panel(highlighted, title='JSON')
            self.assertEqual('\x1b[' in stream.getvalue(), force)
            self.assertIn(source, strip_ansi(stream.getvalue()))
