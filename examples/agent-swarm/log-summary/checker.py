"""Trusted, standard-library-only acceptance checks for log_summary.py."""

import importlib.util
from pathlib import Path


def main() -> int:
    source = Path.cwd() / "log_summary.py"
    spec = importlib.util.spec_from_file_location("candidate_log_summary", source)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load candidate module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    summarize = module.summarize_log

    expected = {
        "total_lines": 4,
        "counts": {"INFO": 1, "WARN": 1, "ERROR": 2},
    }
    assert summarize("INFO: started\nWARN: slow\nERROR: lost\nERROR: retry") == expected
    assert summarize("\nINFO: ready\n\n") == {
        "total_lines": 1,
        "counts": {"INFO": 1, "WARN": 0, "ERROR": 0},
    }
    assert summarize("") == {
        "total_lines": 0,
        "counts": {"INFO": 0, "WARN": 0, "ERROR": 0},
    }
    for text, line in (("INFO: ok\nBOGUS: no", 2), ("malformed", 1)):
        try:
            summarize(text)
        except ValueError as error:
            assert str(line) in str(error), (line, error)
        else:
            raise AssertionError(f"expected ValueError for line {line}")
    print("PASS: log summary behavior")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
