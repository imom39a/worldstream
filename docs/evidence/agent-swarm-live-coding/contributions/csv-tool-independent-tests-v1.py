"""Independent behavioral tests for csv_tool.py.

Run from the directory containing csv_tool.py:
    python -m unittest -v path/to/this_file.py
"""

import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path.cwd()
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import csv_tool

CLI = ROOT / "csv_tool.py"


class ParseRecordsTests(unittest.TestCase):
    def test_01_simple_records(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\nAda,first\nBob,second\n"),
            [{"name": "Ada", "note": "first"}, {"name": "Bob", "note": "second"}],
        )

    def test_02_quoted_comma(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"one, two"\n'),
            [{"name": "Ada", "note": "one, two"}],
        )

    def test_03_doubled_quote(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\n"A ""Ace""","said ""hello"""\n'),
            [{"name": 'A "Ace"', "note": 'said "hello"'}],
        )

    def test_04_embedded_newlines(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"line 1\nline 2"\n'),
            [{"name": "Ada", "note": "line 1\nline 2"}],
        )

    def test_05_crlf_input(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\r\nAda,hello\r\nBob,bye\r\n"),
            [{"name": "Ada", "note": "hello"}, {"name": "Bob", "note": "bye"}],
        )

    def test_06_unicode(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\nZoë,東京 🌍\n"),
            [{"name": "Zoë", "note": "東京 🌍"}],
        )

    def test_07_empty_fields(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\n,\nAda,\n,note\n"),
            [
                {"name": "", "note": ""},
                {"name": "Ada", "note": ""},
                {"name": "", "note": "note"},
            ],
        )

    def test_08_missing_header(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("")

    def test_09_wrong_header_names(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("person,comment\nAda,hello\n")

    def test_10_reversed_header(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("note,name\nhello,Ada\n")

    def test_11_extra_header_column(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note,age\nAda,hello,42\n")

    def test_12_unclosed_quote(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records('name,note\nAda,"unfinished\n')

    def test_13_junk_after_closing_quote(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records('name,note\nAda,"hello"junk\n')

    def test_14_too_few_columns(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note\nAda\n")

    def test_15_too_many_columns(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note\nAda,hello,extra\n")


class FormatRecordsTests(unittest.TestCase):
    def test_16_simple_output_is_exact_and_lf_terminated(self):
        self.assertEqual(
            csv_tool.format_records([{"name": "Ada", "note": "hello"}]),
            "name,note\nAda,hello\n",
        )

    def test_17_quotes_commas_and_newlines_round_trip(self):
        records = [{"name": 'A, "Ace"', "note": "line 1\nline 2"}]
        text = csv_tool.format_records(records)
        self.assertNotIn("\r", text)
        self.assertTrue(text.startswith("name,note\n"))
        self.assertEqual(csv_tool.parse_records(text), records)

    def test_18_unicode_and_empty_values_round_trip(self):
        records = [{"name": "雪", "note": ""}, {"name": "", "note": "café 🐍"}]
        text = csv_tool.format_records(records)
        self.assertEqual(csv_tool.parse_records(text), records)
        self.assertTrue(text.endswith("\n"))

    def test_19_missing_key(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "Ada"}])

    def test_20_extra_key(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "Ada", "note": "hello", "age": "42"}])

    def test_21_non_string_name(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": 123, "note": "hello"}])

    def test_22_non_string_note(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "Ada", "note": None}])


class JsonAndCliTests(unittest.TestCase):
    def test_23_to_json_returns_valid_json_with_strings(self):
        result = csv_tool.to_json("name,note\n001,true\n")
        self.assertIsInstance(result, str)
        self.assertEqual(json.loads(result), [{"name": "001", "note": "true"}])

    def test_24_to_json_preserves_quotes_newlines_and_unicode(self):
        text = 'name,note\n"Zoë, 雪","line 1\n""quoted"" 🐍"\n'
        self.assertEqual(
            json.loads(csv_tool.to_json(text)),
            [{"name": "Zoë, 雪", "note": 'line 1\n"quoted" 🐍'}],
        )

    def test_25_cli_success(self):
        proc = subprocess.run(
            [sys.executable, str(CLI)],
            input="name,note\nAda,hello\n",
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(proc.stderr, "")
        self.assertEqual(json.loads(proc.stdout), [{"name": "Ada", "note": "hello"}])

    def test_26_cli_invalid_input(self):
        proc = subprocess.run(
            [sys.executable, str(CLI)],
            input='name,note\nAda,"unfinished\n',
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 2)
        self.assertEqual(proc.stdout, "")
        self.assertTrue(proc.stderr.strip(), "stderr should contain a useful error message")
        self.assertNotIn("Traceback", proc.stderr)


if __name__ == "__main__":
    unittest.main()
