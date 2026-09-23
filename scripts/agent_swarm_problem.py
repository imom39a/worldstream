"""Bounded user-authored single-file problem for the managed live Swarm."""

from __future__ import annotations

import hashlib
import json
import re
import unicodedata
from dataclasses import dataclass
from pathlib import Path

from agent_swarm_autonomous_delivery import AutonomousDeliveryTrial
from agent_swarm_challenge import require

MAX_SOURCE_BYTES = 8192
MAX_CHECKER_BYTES = 4096
BASE_CONSTRAINT_COUNT = 5  # Four shared instructions plus continuation.
SECRET_PREFIXES = ("bearer ", "sk-", "sk_", "ghp_", "github_pat_", "xoxb-", "xoxp-")


def _setup_text(value: object, label: str, limit: int) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be nonempty text")
    if len(value.encode("utf-8")) > limit:
        raise ValueError(f"{label} exceeds {limit} UTF-8 bytes")
    if any(unicodedata.category(char) == "Cc" for char in value):
        raise ValueError(f"{label} contains a setup-forbidden control character")
    if (
        "${" in value
        or "$(" in value
        or value.strip().lower().startswith(SECRET_PREFIXES)
    ):
        raise ValueError(f"{label} contains a setup-forbidden string")
    return value


def _input_file(base: Path, relative: object, label: str) -> Path:
    if not isinstance(relative, str) or not relative:
        raise ValueError(f"{label} must be a relative file path")
    path = Path(relative)
    if path.is_absolute() or any(
        part in ("", ".", "..") for part in relative.split("/")
    ):
        raise ValueError(f"{label} must stay under the problem directory")
    current = base
    for part in path.parts:
        current = current / part
        if current.is_symlink():
            raise ValueError(f"{label} cannot traverse a symlink")
    if not current.is_file() or not current.resolve().is_relative_to(base):
        raise ValueError(f"{label} must be a regular file under the problem directory")
    return current


def source_constraints(source: bytes) -> list[str]:
    """Carry exact UTF-8 starter bytes as setup-safe JSON string literals."""
    text = source.decode("utf-8")
    if "${" in text or "$(" in text or text.strip().lower().startswith(SECRET_PREFIXES):
        raise ValueError("initial source contains a setup-forbidden string")
    _setup_text(
        text.replace("\n", " ").replace("\r", " "), "initial source", MAX_SOURCE_BYTES
    )
    chunks: list[str] = []
    chunk = ""
    for char in text:
        if chunk and len(json.dumps(chunk + char, ensure_ascii=True).encode()) > 1800:
            chunks.append(chunk)
            chunk = ""
        chunk += char
    if chunk:
        chunks.append(chunk)
    constraints = [
        f"Source part {index}/{len(chunks)}, JSON string: "
        + json.dumps(part, ensure_ascii=True)
        for index, part in enumerate(chunks, 1)
    ]
    if (
        "".join(
            json.loads(item.split("JSON string: ", 1)[1]) for item in constraints
        ).encode()
        != source
    ):
        raise ValueError("source chunks did not preserve exact bytes")
    for item in constraints:
        _setup_text(item, "source constraint", 2048)
    return constraints


@dataclass(frozen=True)
class Problem:
    config_path: Path
    config_sha256: str
    goal: str
    constraints: tuple[str, ...]
    acceptance_criterion: str
    target: str
    source: bytes
    checker: str
    require_parallel: bool
    source_parts: tuple[str, ...]

    def goal_text(self) -> str:
        text = (
            f"{self.goal} The initial {self.target} source is supplied as numbered JSON "
            "string constraints. Decode each string and concatenate in order without "
            f"separators; its exact UTF-8 SHA256 is {hashlib.sha256(self.source).hexdigest()}. "
            "The same bytes are registered as resource version 1. The production planner "
            "owns work, checks, independent review, and delivery."
        )
        return _setup_text(text, "effective goal", 4096)

    def summary(self) -> dict[str, object]:
        return {
            "config_sha256": self.config_sha256,
            "target": self.target,
            "initial_source_sha256": "sha256:"
            + hashlib.sha256(self.source).hexdigest(),
            "checker_sha256": "sha256:"
            + hashlib.sha256(self.checker.encode()).hexdigest(),
            "goal_bytes": len(self.goal_text().encode()),
            "source_parts": len(self.source_parts),
            "constraints": BASE_CONSTRAINT_COUNT
            + len(self.constraints)
            + len(self.source_parts),
            "require_parallel": self.require_parallel,
        }


def load_problem(path: Path) -> Problem:
    if (
        not path.is_absolute()
        or any(part.is_symlink() for part in (path, *path.parents))
        or not path.is_file()
    ):
        raise ValueError("problem config must be an absolute regular JSON file")
    data = path.read_bytes()
    if len(data) > 16 * 1024:
        raise ValueError("problem config exceeds 16 KiB")

    def unique_fields(pairs):
        fields = {}
        for key, value in pairs:
            if key in fields:
                raise ValueError(f"duplicate problem JSON field: {key}")
            fields[key] = value
        return fields

    document = json.loads(data, object_pairs_hook=unique_fields)
    required = {
        "goal",
        "constraints",
        "acceptance_criterion",
        "target",
        "initial_source_file",
        "checker_file",
    }
    if (
        not isinstance(document, dict)
        or not required <= document.keys()
        or document.keys() - required - {"require_parallel"}
    ):
        raise ValueError("problem JSON requires the exact documented fields")
    base = path.parent.resolve(strict=True)
    target = document["target"]
    if (
        not isinstance(target, str)
        or re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]*\.(py|txt|md|json)", target) is None
    ):
        raise ValueError("target must be one portable .py, .txt, .md or .json basename")
    constraints = document["constraints"]
    if not isinstance(constraints, list) or len(constraints) > 32:
        raise ValueError("constraints must be a list of at most 32 strings")
    constraints = tuple(_setup_text(item, "constraint", 2048) for item in constraints)
    goal = _setup_text(document["goal"], "goal", 4096)
    criterion = _setup_text(
        document["acceptance_criterion"], "acceptance criterion", 2048
    )
    source_path = _input_file(
        base, document["initial_source_file"], "initial_source_file"
    )
    checker_path = _input_file(base, document["checker_file"], "checker_file")
    source = source_path.read_bytes()
    if not source or len(source) > MAX_SOURCE_BYTES:
        raise ValueError("initial source must contain 1 to 8192 UTF-8 bytes")
    checker_bytes = checker_path.read_bytes()
    if not checker_bytes or len(checker_bytes) > MAX_CHECKER_BYTES:
        raise ValueError("checker must contain 1 to 4096 UTF-8 bytes")
    checker = checker_bytes.decode("utf-8")
    try:
        compile(checker, str(checker_path), "exec")
    except SyntaxError as error:
        raise ValueError(
            f"checker_file has invalid Python syntax at line {error.lineno}"
        ) from None
    parallel = document.get("require_parallel", False)
    if not isinstance(parallel, bool):
        raise TypeError("require_parallel must be boolean")
    parts = tuple(source_constraints(source))
    problem = Problem(
        path,
        "sha256:" + hashlib.sha256(data).hexdigest(),
        goal,
        constraints,
        criterion,
        target,
        source,
        checker,
        parallel,
        parts,
    )
    problem.goal_text()
    if BASE_CONSTRAINT_COUNT + len(constraints) + len(parts) > 64:
        raise ValueError("effective constraints exceed the 64-item Room limit")
    return problem


class ProblemTrial(AutonomousDeliveryTrial):
    """Custom task through the unchanged production autonomous delivery flow."""

    def __init__(self, harness, *, problem: Problem, **kwargs):
        self.problem = problem
        super().__init__(harness, **kwargs)

    def target_name(self):
        return self.problem.target

    def resource_id(self):
        return "problem-target"

    def initial_source(self):
        return self.problem.source.decode("utf-8")

    def goal_text(self):
        return self.problem.goal_text()

    def criterion_text(self):
        return self.problem.acceptance_criterion

    def additional_constraints(self):
        return [*self.problem.constraints, *self.problem.source_parts]

    def checker_text(self):
        return self.problem.checker

    def require_parallel(self):
        return self.problem.require_parallel

    def run(self):
        report = super().run()
        report["schema"] = "worldstream/agent-swarm-live-problem-evaluation@1"
        report["problem"] = self.problem.summary()
        report["checker_sha256"] = report.pop("fixed_checker_sha256")
        report.pop("fixed_check_count")
        report["limits"] = [
            "One bounded single-file UTF-8 deliverable under an explicit local delivery policy.",
            "The evaluator supplied the starter source, criterion and trusted checker; the planner owned delivery decisions.",
            "No cross-run learning or portable release qualification is claimed.",
        ]
        require(
            Path(report["delivered_path"]).name == self.problem.target,
            "delivered target differs from problem config",
        )
        return report
