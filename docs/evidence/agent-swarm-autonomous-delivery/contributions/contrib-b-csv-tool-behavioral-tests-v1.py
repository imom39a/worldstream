import json
import subprocess
import sys
import unittest
from pathlib import Path

import csv_tool


class ParseRecordsTests(unittest.TestCase):
    def test_basic_records(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\nAda,hello\nBob,world\n"),
            [{"name": "Ada", "note": "hello"}, {"name": "Bob", "note": "world"}],
        )

    def test_quoted_comma(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"hello, world"\n'),
            [{"name": "Ada", "note": "hello, world"}],
        )

    def test_doubled_quote(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"She said ""hi"""\n'),
            [{"name": "Ada", "note": 'She said "hi"'}],
        )

    def test_embedded_lf(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"line 1\nline 2"\n'),
            [{"name": "Ada", "note": "line 1\nline 2"}],
        )

    def test_crlf_records_and_embedded_crlf(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\r\nAda,"line 1\r\nline 2"\r\nBob,ok\r\n'),
            [
                {"name": "Ada", "note": "line 1\r\nline 2"},
                {"name": "Bob", "note": "ok"},
            ],
        )

    def test_unicode(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\nZoë,雪だるま ☃\n"),
            [{"name": "Zoë", "note": "雪だるま ☃"}],
        )

    def test_empty_fields(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\n,\nAda,\n,note\n"),
            [
                {"name": "", "note": ""},
                {"name": "Ada", "note": ""},
                {"name": "", "note": "note"},
            ],
        )

    def test_missing_header(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("")

    def test_wrong_header(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("note,name\nhello,Ada\n")

    def test_extra_header_column(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note,other\nAda,hello,x\n")

    def test_unclosed_quote(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records('name,note\nAda,"unfinished\n')

    def test_too_many_columns(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note\nAda,hello,extra\n")

    def test_too_few_columns(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note\nAda\n")


class FormatRecordsTests(unittest.TestCase):
    def test_basic_output_and_lf_terminators(self):
        self.assertEqual(
            csv_tool.format_records([{"name": "Ada", "note": "hello"}]),
            "name,note\nAda,hello\n",
        )

    def test_quotes_comma(self):
        self.assertEqual(
            csv_tool.format_records([{"name": "Ada", "note": "hello, world"}]),
            'name,note\nAda,"hello, world"\n',
        )

    def test_doubles_quotes(self):
        self.assertEqual(
            csv_tool.format_records([{"name": "Ada", "note": 'say "hi"'}]),
            'name,note\nAda,"say ""hi"""\n',
        )

    def test_preserves_newlines_and_unicode(self):
        formatted = csv_tool.format_records([{"name": "Zoë", "note": "雪\r\nline 2"}])
        self.assertEqual(formatted, 'name,note\nZoë,"雪\r\nline 2"\n')
        self.assertEqual(
            csv_tool.parse_records(formatted),
            [{"name": "Zoë", "note": "雪\r\nline 2"}],
        )

    def test_empty_records_and_fields(self):
        self.assertEqual(csv_tool.format_records([]), "name,note\n")
        self.assertEqual(
            csv_tool.format_records([{"name": "", "note": ""}]),
            "name,note\n,\n",
        )

    def test_missing_key_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "Ada"}])

    def test_extra_key_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "Ada", "note": "hi", "extra": "x"}])

    def test_non_string_name_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": 7, "note": "hi"}])

    def test_non_string_note_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "Ada", "note": None}])


class JsonTests(unittest.TestCase):
    def test_json_preserves_all_field_strings(self):
        source = 'name,note\n"A, B","line 1\nline 2 and ""quotes"""\nZoë,雪\n'
        result = csv_tool.to_json(source)
        self.assertEqual(
            json.loads(result),
            [
                {"name": "A, B", "note": 'line 1\nline 2 and "quotes"'},
                {"name": "Zoë", "note": "雪"},
            ],
        )
        self.assertIsInstance(result, str)

    def test_invalid_csv_is_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.to_json("bad,header\nx,y\n")


class CliTests(unittest.TestCase):
    @staticmethod
    def run_cli(source):
        return subprocess.run(
            [sys.executable, str(Path(csv_tool.__file__).resolve())],
            input=source,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_success_stdout_only_and_exit_zero(self):
        proc = self.run_cli('name,note\nZoë,"hello, 雪"\n')
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(proc.stderr, "")
        self.assertEqual(
            json.loads(proc.stdout),
            [{"name": "Zoë", "note": "hello, 雪"}],
        )

    def test_invalid_input_stderr_no_traceback_and_exit_two(self):
        proc = self.run_cli('name,note\nAda,"unfinished\n')
        self.assertEqual(proc.returncode, 2)
        self.assertEqual(proc.stdout, "")
        self.assertTrue(proc.stderr.strip())
        self.assertNotIn("Traceback", proc.stderr)


if __name__ == "__main__":
    unittest.main()
