"""Strict CSV record conversion utilities and command-line interface."""

import csv
import io
import json
import sys
from collections.abc import Mapping


_HEADER = ["name", "note"]


def _validate_csv_syntax(text):
    """Reject quote placement that csv.reader otherwise accepts leniently."""
    in_quotes = False
    at_field_start = True
    just_closed_quote = False
    line = 1
    i = 0

    while i < len(text):
        char = text[i]

        if in_quotes:
            if char == '"':
                if i + 1 < len(text) and text[i + 1] == '"':
                    i += 2
                    continue
                in_quotes = False
                just_closed_quote = True
            elif char == "\n":
                line += 1
            elif char == "\r" and (i + 1 == len(text) or text[i + 1] != "\n"):
                line += 1
            i += 1
            continue

        if just_closed_quote:
            if char == ",":
                at_field_start = True
                just_closed_quote = False
            elif char == "\n":
                line += 1
                at_field_start = True
                just_closed_quote = False
            elif char == "\r":
                line += 1
                at_field_start = True
                just_closed_quote = False
                if i + 1 < len(text) and text[i + 1] == "\n":
                    i += 1
            else:
                raise ValueError(
                    "malformed CSV quoting on line {}: unexpected character after closing quote".format(line)
                )
            i += 1
            continue

        if char == '"':
            if not at_field_start:
                raise ValueError(
                    "malformed CSV quoting on line {}: quote in unquoted field".format(line)
                )
            in_quotes = True
            at_field_start = False
        elif char == ",":
            at_field_start = True
        elif char == "\n":
            line += 1
            at_field_start = True
        elif char == "\r":
            line += 1
            at_field_start = True
            if i + 1 < len(text) and text[i + 1] == "\n":
                i += 1
        else:
            at_field_start = False
        i += 1

    if in_quotes:
        raise ValueError("malformed CSV quoting: unterminated quoted field")


def parse_records(text):
    """Parse CSV text having exactly the header ``name,note``."""
    if not isinstance(text, str):
        raise ValueError("CSV input must be a string")

    _validate_csv_syntax(text)
    stream = io.StringIO(text, newline="")
    reader = csv.reader(stream, strict=True)

    try:
        try:
            header = next(reader)
        except StopIteration:
            raise ValueError("missing CSV header; expected 'name,note'") from None

        if header != _HEADER:
            raise ValueError("invalid CSV header; expected 'name,note'")

        records = []
        for row in reader:
            if len(row) != 2:
                raise ValueError(
                    "invalid column count on CSV record ending at line {}: expected 2, got {}".format(
                        reader.line_num, len(row)
                    )
                )
            records.append({"name": row[0], "note": row[1]})
        return records
    except csv.Error as error:
        raise ValueError("malformed CSV near line {}: {}".format(reader.line_num, error)) from None


def _format_row(values):
    # CRLF here makes the writer quote fields containing either CR or LF. The
    # terminator alone is then replaced, preserving embedded newlines exactly.
    buffer = io.StringIO(newline="")
    writer = csv.writer(buffer, lineterminator="\r\n")
    writer.writerow(values)
    rendered = buffer.getvalue()
    return rendered[:-2] + "\n"


def format_records(records):
    """Serialize valid name/note mappings as CSV with LF record endings."""
    output = ["name,note\n"]

    try:
        iterator = iter(records)
    except TypeError:
        raise ValueError("records must be an iterable of mappings") from None

    for index, record in enumerate(iterator, start=1):
        if not isinstance(record, Mapping):
            raise ValueError("record {} must be a mapping".format(index))
        if set(record.keys()) != {"name", "note"}:
            raise ValueError(
                "record {} must contain exactly the keys 'name' and 'note'".format(index)
            )
        name = record["name"]
        note = record["note"]
        if not isinstance(name, str) or not isinstance(note, str):
            raise ValueError("record {} values must be strings".format(index))
        output.append(_format_row((name, note)))

    return "".join(output)


def to_json(text):
    """Convert valid CSV text to JSON without changing field strings."""
    return json.dumps(parse_records(text), ensure_ascii=False)


def main():
    try:
        result = to_json(sys.stdin.read())
    except ValueError as error:
        sys.stderr.write("csv_tool.py: invalid CSV: {}\n".format(error))
        return 2

    sys.stdout.write(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
