"""Independent behavioral checks for csv_tool.py.

Run from the directory containing csv_tool.py, or set CSV_TOOL_PATH.
"""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest


MODULE_PATH = Path(os.environ.get("CSV_TOOL_PATH", Path.cwd() / "csv_tool.py")).resolve()
_SPEC = importlib.util.spec_from_file_location("csv_tool_under_test", MODULE_PATH)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
csv_tool = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(csv_tool)


class ParseRecordsTests(unittest.TestCase):
    def test_basic_records(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\nAda,hello\nBob,world\n"),
            [{"name": "Ada", "note": "hello"}, {"name": "Bob", "note": "world"}],
        )

    def test_header_only_is_empty_record_list(self):
        self.assertEqual(csv_tool.parse_records("name,note\n"), [])

    def test_quoted_comma(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"one,two"\n'),
            [{"name": "Ada", "note": "one,two"}],
        )

    def test_doubled_quotes(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"said ""hi"""\n'),
            [{"name": "Ada", "note": 'said "hi"'}],
        )

    def test_embedded_lf(self):
        self.assertEqual(
            csv_tool.parse_records('name,note\nAda,"first\nsecond"\n'),
            [{"name": "Ada", "note": "first\nsecond"}],
        )

    def test_crlf_records_and_embedded_crlf(self):
        text = 'name,note\r\nAda,"first\r\nsecond"\r\nBob,done\r\n'
        self.assertEqual(
            csv_tool.parse_records(text),
            [
                {"name": "Ada", "note": "first\r\nsecond"},
                {"name": "Bob", "note": "done"},
            ],
        )

    def test_unicode_and_empty_fields(self):
        self.assertEqual(
            csv_tool.parse_records("name,note\n雪,☃️\n,\n"),
            [{"name": "雪", "note": "☃️"}, {"name": "", "note": ""}],
        )

    def test_missing_header(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("")

    def test_wrong_header(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("note,name\nx,y\n")

    def test_bom_makes_header_nonexact(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("\ufeffname,note\nx,y\n")

    def test_too_few_columns(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note\nonly-one\n")

    def test_too_many_columns(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records("name,note\na,b,c\n")

    def test_quote_inside_unquoted_field_is_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records('name,note\na"b,c\n')

    def test_character_after_closing_quote_is_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records('name,note\n"a"x,b\n')

    def test_unterminated_quoted_field_is_rejected(self):
        with self.assertRaises(ValueError):
            csv_tool.parse_records('name,note\n"a,b\n')


class FormatRecordsTests(unittest.TestCase):
    def test_empty_iterable_emits_header(self):
        self.assertEqual(csv_tool.format_records([]), "name,note\n")

    def test_complex_values_round_trip(self):
        records = [
            {"name": 'A,"雪"', "note": "line 1\nline 2"},
            {"name": "", "note": ""},
        ]
        rendered = csv_tool.format_records(records)
        self.assertEqual(csv_tool.parse_records(rendered), records)

    def test_uses_lf_record_terminators_and_preserves_embedded_crlf(self):
        rendered = csv_tool.format_records([{"name": "x", "note": "a\r\nb"}])
        self.assertEqual(rendered, 'name,note\nx,"a\r\nb"\n')

    def test_accepts_generator(self):
        records = ({"name": str(i), "note": "n"} for i in range(2))
        self.assertEqual(
            csv_tool.format_records(records),
            "name,note\n0,n\n1,n\n",
        )

    def test_rejects_missing_key(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "x"}])

    def test_rejects_extra_key(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([{"name": "x", "note": "y", "extra": "z"}])

    def test_rejects_non_string_values(self):
        for record in (
            {"name": 1, "note": "y"},
            {"name": "x", "note": None},
        ):
            with self.subTest(record=record), self.assertRaises(ValueError):
                csv_tool.format_records([record])

    def test_rejects_non_mapping_record(self):
        with self.assertRaises(ValueError):
            csv_tool.format_records([["x", "y"]])


class JsonTests(unittest.TestCase):
    def test_to_json_preserves_strings(self):
        text = 'name,note\n雪,"comma, quote "" and\nnewline"\n'
        self.assertEqual(
            json.loads(csv_tool.to_json(text)),
            [{"name": "雪", "note": 'comma, quote " and\nnewline'}],
        )


class CliTests(unittest.TestCase):
    def test_success_stdout_and_exit_code(self):
        proc = subprocess.run(
            [sys.executable, str(MODULE_PATH)],
            input="name,note\n雪,hello\n",
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(proc.stderr, "")
        self.assertEqual(json.loads(proc.stdout), [{"name": "雪", "note": "hello"}])

    def test_invalid_csv_stderr_only_and_exit_two(self):
        proc = subprocess.run(
            [sys.executable, str(MODULE_PATH)],
            input='name,note\na"b,c\n',
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 2)
        self.assertEqual(proc.stdout, "")
        self.assertTrue(proc.stderr.startswith("error: "), proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)


if __name__ == "__main__":
    unittest.main()
