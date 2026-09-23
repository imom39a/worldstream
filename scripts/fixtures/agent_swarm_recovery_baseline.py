"""Strict CSV record conversion library and command-line tool."""

import csv
import io
import json
import sys
from typing import Iterable, Mapping


_HEADER = ["name", "note"]
_KEYS = frozenset(_HEADER)


def _validate_quoting(text: str) -> None:
    """Reject quote placement that csv.reader otherwise accepts permissively."""
    field_start = True
    in_quotes = False
    after_quote = False
    index = 0

    while index < len(text):
        char = text[index]

        if in_quotes:
            if after_quote:
                if char == '"':
                    after_quote = False
                elif char == ',':
                    in_quotes = False
                    after_quote = False
                    field_start = True
                elif char == '\r' or char == '\n':
                    in_quotes = False
                    after_quote = False
                    field_start = True
                    if char == '\r' and index + 1 < len(text) and text[index + 1] == '\n':
                        index += 1
                else:
                    raise ValueError(
                        "malformed CSV: unexpected character after closing quote"
                    )
            elif char == '"':
                after_quote = True
        else:
            if char == '"':
                if not field_start:
                    raise ValueError(
                        "malformed CSV: quote inside an unquoted field"
                    )
                in_quotes = True
                after_quote = False
                field_start = False
            elif char == ',':
                field_start = True
            elif char == '\r' or char == '\n':
                field_start = True
                if char == '\r' and index + 1 < len(text) and text[index + 1] == '\n':
                    index += 1
            else:
                field_start = False

        index += 1

    if in_quotes and not after_quote:
        raise ValueError("malformed CSV: unterminated quoted field")


def parse_records(text: str) -> list[dict[str, str]]:
    """Parse CSV having exactly the header ``name,note``."""
    if not isinstance(text, str):
        raise ValueError("CSV input must be a string")

    reader = csv.reader(io.StringIO(text, newline=""), strict=True)

    try:
        header = next(reader)
    except StopIteration:
        raise ValueError("missing CSV header; expected name,note") from None
    except csv.Error as exc:
        raise ValueError(f"malformed CSV header: {exc}") from None

    if header != _HEADER:
        raise ValueError("wrong CSV header; expected name,note")

    records: list[dict[str, str]] = []
    try:
        for row_number, row in enumerate(reader, start=2):
            if len(row) != 2:
                raise ValueError(
                    f"wrong column count on row {row_number}: expected 2, got {len(row)}"
                )
            records.append({"name": row[0], "note": row[1]})
    except csv.Error as exc:
        raise ValueError(f"malformed CSV: {exc}") from None

    return records


def format_records(records: Iterable[Mapping[str, str]]) -> str:
    """Serialize validated name/note records using LF line terminators."""
    output = io.StringIO(newline="")
    writer = csv.writer(output, lineterminator="\n")
    writer.writerow(_HEADER)

    try:
        iterator = iter(records)
    except TypeError:
        raise ValueError("records must be an iterable of mappings") from None

    for record_number, record in enumerate(iterator, start=1):
        if not isinstance(record, Mapping):
            raise ValueError(f"record {record_number} must be a mapping")
        try:
            keys = frozenset(record.keys())
        except (AttributeError, TypeError):
            raise ValueError(
                f"record {record_number} must have exactly name and note keys"
            ) from None
        if keys != _KEYS or len(record) != 2:
            raise ValueError(
                f"record {record_number} must have exactly name and note keys"
            )

        name = record["name"]
        note = record["note"]
        if not isinstance(name, str) or not isinstance(note, str):
            raise ValueError(
                f"record {record_number} name and note values must be strings"
            )
        writer.writerow([name, note])

    return output.getvalue()


def to_json(text: str) -> str:
    """Convert valid CSV text to JSON without changing field strings."""
    return json.dumps(parse_records(text), ensure_ascii=False)


def main() -> int:
    try:
        result = to_json(sys.stdin.read())
    except ValueError as exc:
        sys.stderr.write(f"error: {exc}\n")
        return 2

    sys.stdout.write(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
