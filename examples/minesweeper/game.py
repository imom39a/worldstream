"""Pure Minesweeper game logic for the LLM-vs-JEV comparison demo.

Unlike the Tower of Hanoi demo, a Minesweeper decision needs no move history:
the authoritative board at the current round is the entire input. That makes a
direct LLM-vs-JEV comparison fair, because each engine sees exactly the same
closed candidate set and neither has to reconstruct prior state.
"""

from __future__ import annotations

import random

HIDDEN = "?"
FLAG = "F"
MINE = "*"

MIN_DIMENSION = 2
MAX_DIMENSION = 500
# Absolute ceiling; the real limit for any board is width * height - 1, so a
# 200x200 board can hold 9000 mines and a 500x500 board can hold up to 249999.
MAX_MINES = 250_000
# Bounded candidate set: large boards can have thousands of frontier cells, but
# a model should never be asked to rank more than a handful. Keeping the set
# small and identical for every player bounds token cost and keeps the
# comparison fair.
MAX_CANDIDATES = 24
# Above this many cells a board is considered "large": the full grid is omitted
# from the engine state and only the frontier is sent.
LARGE_BOARD_CELLS = 4096


class MinesweeperError(Exception):
    """Raised when a game configuration or move is invalid."""


def cell_label(row: int, col: int) -> str:
    """One-based, reference-safe label such as ``r3c5``."""
    return f"r{row + 1}c{col + 1}"


def parse_cell_label(label: object) -> tuple[int, int] | None:
    """Parse ``r3c5`` back into zero-based coordinates, or None if malformed."""
    if not isinstance(label, str) or not label.startswith("r") or "c" not in label:
        return None
    row_text, _, col_text = label[1:].partition("c")
    try:
        row, col = int(row_text), int(col_text)
    except ValueError:
        return None
    if row < 1 or col < 1:
        return None
    return row - 1, col - 1


class MinesweeperGame:
    """One deterministic Minesweeper board.

    Mines are placed lazily on the first reveal so the opening click is always
    safe. Both Rooms in a comparison share one seed and therefore one board.
    """

    def __init__(self, width: int, height: int, mines: int, seed: int) -> None:
        if isinstance(width, bool) or not isinstance(width, int):
            raise MinesweeperError("width_invalid")
        if isinstance(height, bool) or not isinstance(height, int):
            raise MinesweeperError("height_invalid")
        if isinstance(mines, bool) or not isinstance(mines, int):
            raise MinesweeperError("mines_invalid")
        if not MIN_DIMENSION <= width <= MAX_DIMENSION:
            raise MinesweeperError("width_invalid")
        if not MIN_DIMENSION <= height <= MAX_DIMENSION:
            raise MinesweeperError("height_invalid")
        if not 1 <= mines <= min(MAX_MINES, width * height - 1):
            raise MinesweeperError("mines_invalid")
        self.width = width
        self.height = height
        self.mines = mines
        self.seed = seed
        self.rng = random.Random(seed)
        self.mine_cells: set[tuple[int, int]] = set()
        self.revealed: set[tuple[int, int]] = set()
        self.flagged: set[tuple[int, int]] = set()
        self.placed = False
        self.phase = "ready"
        self.moves = 0
        self.boom: tuple[int, int] | None = None

    # -- geometry -----------------------------------------------------------

    def in_bounds(self, row: int, col: int) -> bool:
        return 0 <= row < self.height and 0 <= col < self.width

    def neighbors(self, row: int, col: int) -> list[tuple[int, int]]:
        result: list[tuple[int, int]] = []
        for dr in (-1, 0, 1):
            for dc in (-1, 0, 1):
                if dr == 0 and dc == 0:
                    continue
                nr, nc = row + dr, col + dc
                if self.in_bounds(nr, nc):
                    result.append((nr, nc))
        return result

    def adjacent_mines(self, row: int, col: int) -> int:
        return sum(1 for cell in self.neighbors(row, col) if cell in self.mine_cells)

    # -- state --------------------------------------------------------------

    @property
    def safe_total(self) -> int:
        return self.width * self.height - self.mines

    @property
    def is_terminal(self) -> bool:
        return self.phase in {"won", "lost", "time_limit"}

    def token_grid(self) -> list[list[str]]:
        """Public board as tokens: ``?`` hidden, ``F`` flag, ``*`` mine, else digit."""
        grid: list[list[str]] = []
        for row in range(self.height):
            tokens: list[str] = []
            for col in range(self.width):
                cell = (row, col)
                if cell in self.revealed:
                    tokens.append(MINE if cell in self.mine_cells else str(self.adjacent_mines(row, col)))
                elif cell in self.flagged:
                    tokens.append(FLAG)
                else:
                    tokens.append(HIDDEN)
            grid.append(tokens)
        return grid

    def candidates(self, limit: int = MAX_CANDIDATES) -> list[dict[str, object]]:
        """Ordered hidden cells both engines may choose this round.

        Frontier cells (adjacent to a revealed number) come first, ordered by how
        many revealed neighbours constrain them, then unconstrained guesses in
        row/col order. The list is identical for both engines.
        """
        if self.phase in {"won", "lost", "time_limit"}:
            return []
        cells = [
            (row, col)
            for row in range(self.height)
            for col in range(self.width)
            if (row, col) not in self.revealed and (row, col) not in self.flagged
        ]
        cells.sort(key=lambda cell: (-len(self.revealed_neighbors(*cell)), cell[0], cell[1]))
        return [
            {
                "cell": cell_label(row, col),
                "row": row,
                "col": col,
                "frontier": bool(self.revealed_neighbors(row, col)),
                "constraints": self.cell_constraints(row, col),
            }
            for row, col in cells[:limit]
        ]

    def revealed_neighbors(self, row: int, col: int) -> list[tuple[int, int]]:
        return [cell for cell in self.neighbors(row, col) if cell in self.revealed]

    def cell_constraints(self, row: int, col: int) -> str:
        """Compact deduction context from the cell's revealed neighbours.

        Format is ``r10c20=2(3h)``: the revealed number and how many hidden cells
        still surround it. Kept short because this text is repeated for every
        offered candidate in the engine prompt.
        """
        parts: list[str] = []
        for nr, nc in self.neighbors(row, col):
            if (nr, nc) not in self.revealed or (nr, nc) in self.mine_cells:
                continue
            hidden_around = sum(
                1
                for cell in self.neighbors(nr, nc)
                if cell not in self.revealed and cell not in self.flagged
            )
            parts.append(f"{cell_label(nr, nc)}={self.adjacent_mines(nr, nc)}({hidden_around}h)")
        return " ".join(parts) if parts else "no adjacent numbers"

    def frontier_hidden(self) -> set[tuple[int, int]]:
        """Hidden cells adjacent to at least one revealed number.

        The frontier is the only place a deduction can exist, and it is far
        smaller than the board, so candidate generation scans it instead of every
        hidden cell.
        """
        frontier: set[tuple[int, int]] = set()
        for cell in self.revealed:
            if cell in self.mine_cells:
                continue
            for neighbor in self.neighbors(*cell):
                if neighbor not in self.revealed and neighbor not in self.flagged:
                    frontier.add(neighbor)
        return frontier

    def deduce(self) -> tuple[set[tuple[int, int]], set[tuple[int, int]]]:
        """Cells that the revealed numbers prove safe or prove to be mines.

        Two sound rules are applied to a fixpoint:

        * single constraint: a revealed number whose remaining mine count is zero
          proves all its hidden neighbours safe; one whose count equals its
          hidden neighbours proves them all mines.
        * subset: if constraint A's hidden set is contained in B's, then
          ``remaining_B - remaining_A`` mines lie in ``B - A``; zero proves
          ``B - A`` safe and the full size proves it all mines.

        Code owns this exact logic; the engines only judge the cells that remain
        uncertain.
        """
        proven_safe: set[tuple[int, int]] = set()
        proven_mine: set[tuple[int, int]] = set()
        changed = True
        while changed:
            changed = False
            constraints: list[tuple[frozenset[tuple[int, int]], int]] = []
            for row in range(self.height):
                for col in range(self.width):
                    if (row, col) not in self.revealed or (row, col) in self.mine_cells:
                        continue
                    hidden = [
                        cell
                        for cell in self.neighbors(row, col)
                        if cell not in self.revealed
                        and cell not in self.flagged
                        and cell not in proven_safe
                        and cell not in proven_mine
                    ]
                    if not hidden:
                        continue
                    known = sum(
                        1
                        for cell in self.neighbors(row, col)
                        if cell in self.flagged or cell in proven_mine
                    )
                    remaining = self.adjacent_mines(row, col) - known
                    if remaining == 0:
                        proven_safe.update(hidden)
                        changed = True
                    elif remaining == len(hidden):
                        proven_mine.update(hidden)
                        changed = True
                    else:
                        constraints.append((frozenset(hidden), remaining))
            for index, (hidden_a, remaining_a) in enumerate(constraints):
                for hidden_b, remaining_b in constraints[index + 1 :]:
                    if hidden_a <= hidden_b:
                        difference, delta = hidden_b - hidden_a, remaining_b - remaining_a
                    elif hidden_b <= hidden_a:
                        difference, delta = hidden_a - hidden_b, remaining_a - remaining_b
                    else:
                        continue
                    if not difference or not 0 <= delta <= len(difference):
                        continue
                    if delta == 0:
                        proven_safe.update(difference)
                        changed = True
                    elif delta == len(difference):
                        proven_mine.update(difference)
                        changed = True
        return proven_safe, proven_mine

    def risk_score(
        self, row: int, col: int, known_mines: set[tuple[int, int]] | None = None
    ) -> float:
        """Code-computed mine-likelihood estimate in [0, 1] for one hidden cell.

        Averages the per-constraint mine fraction over the revealed numbers that
        constrain the cell. Cells with no adjacent number fall back to the global
        remaining-mine density. ``known_mines`` lets a caller reuse one deduction
        pass across every candidate instead of recomputing it per cell.
        """
        proven_mine = self.deduce()[1] if known_mines is None else known_mines
        known = self.flagged | proven_mine
        contributions: list[float] = []
        for nr, nc in self.neighbors(row, col):
            if (nr, nc) not in self.revealed or (nr, nc) in self.mine_cells:
                continue
            hidden = [
                cell
                for cell in self.neighbors(nr, nc)
                if cell not in self.revealed and cell not in known
            ]
            if (row, col) not in hidden:
                continue
            remaining = self.adjacent_mines(nr, nc) - sum(
                1 for cell in self.neighbors(nr, nc) if cell in known
            )
            contributions.append(max(0.0, remaining) / len(hidden))
        if contributions:
            return max(0.0, min(1.0, sum(contributions) / len(contributions)))
        hidden_total = sum(
            1
            for r in range(self.height)
            for c in range(self.width)
            if (r, c) not in self.revealed and (r, c) not in known
        )
        if not hidden_total:
            return 1.0
        return max(0.0, min(1.0, (self.mines - len(known)) / hidden_total))

    def offered_candidates(self, limit: int = MAX_CANDIDATES) -> list[dict[str, object]]:
        """The bounded cells a player may choose, with code-derived safety facts.

        Only frontier cells are considered, so a 500x500 board costs the same as
        a small one. Cells proven to be mines are never offered. When any cell is
        proven safe, only proven-safe cells are offered; otherwise the frontier
        guesses are ordered by ascending computed mine risk. The list is
        identical for every player, so a comparison stays fair.
        """
        proven_safe, proven_mine = self.deduce()
        pool = [cell for cell in self.frontier_hidden() if cell not in proven_mine]
        if not pool:
            # No frontier: every hidden cell is unconstrained. Offer a bounded,
            # spread sample rather than the whole board or a biased corner.
            hidden = [
                (row, col)
                for row in range(self.height)
                for col in range(self.width)
                if (row, col) not in self.revealed and (row, col) not in self.flagged
            ]
            step = max(1, len(hidden) // max(1, limit))
            pool = hidden[::step][:limit]
        safe_pool = [cell for cell in pool if cell in proven_safe]
        chosen = safe_pool if safe_pool else pool
        chosen.sort(
            key=lambda cell: (
                0 if cell in proven_safe else 1,
                self.risk_score(*cell, proven_mine),
                cell[0],
                cell[1],
            )
        )
        return [self._candidate(cell, cell in proven_safe, proven_mine) for cell in chosen[:limit]]

    def _candidate(
        self,
        cell: tuple[int, int],
        provably_safe: bool,
        proven_mine: set[tuple[int, int]],
    ) -> dict[str, object]:
        row, col = cell
        return {
            "cell": cell_label(row, col),
            "row": row,
            "col": col,
            "frontier": bool(self.revealed_neighbors(row, col)),
            "provably_safe": provably_safe,
            "mine_risk": round(self.risk_score(row, col, proven_mine), 3),
            "constraints": self.cell_constraints(row, col),
        }

    def agent_state(
        self,
        round_number: int,
        limit: int = MAX_CANDIDATES,
        include_grid: bool | None = None,
    ) -> dict[str, object]:
        """The state object handed to a player.

        The full grid is included only for small boards. On a large board the
        grid would be hundreds of thousands of tokens, so the state carries the
        board summary, the bounded candidate set, and each candidate's local
        constraints instead.
        """
        if include_grid is None:
            include_grid = self.width * self.height <= LARGE_BOARD_CELLS
        offered = self.offered_candidates(limit)
        board: dict[str, object] = {
            "width": self.width,
            "height": self.height,
            "mines": self.mines,
            "mines_remaining": self.mines - len(self.flagged),
            "revealed_count": len(self.revealed),
            "hidden_count": self.width * self.height - len(self.revealed) - len(self.flagged),
            "safe_total": self.safe_total,
        }
        if include_grid:
            board["grid"] = self.token_grid()
        return {
            "game": "minesweeper",
            "rules": (
                "Reveal one hidden cell per turn. A digit counts adjacent mines. "
                "Revealing a mine loses immediately. Reveal every non-mine cell to win. "
                "The first reveal is always safe."
            ),
            "board": board,
            "round": round_number,
            "deductions": {
                "provably_safe": [c["cell"] for c in offered if c["provably_safe"]],
                "provably_mine_excluded": True,
            },
            "candidates": offered,
        }

    def choice_criteria(self, limit: int = MAX_CANDIDATES) -> dict[str, str]:
        """JEV's closed Choice criteria, one compact label per offered candidate."""
        criteria: dict[str, str] = {}
        for candidate in self.offered_candidates(limit):
            if candidate["provably_safe"]:
                note = "provably safe"
            else:
                note = f"risk {candidate['mine_risk']:.2f}"
            criteria[candidate["cell"]] = (
                f"{candidate['cell']} · {note} · {candidate['constraints']}"
            )
        return criteria

    # -- moves --------------------------------------------------------------

    def reveal(self, row: int, col: int) -> str:
        """Reveal one cell. Returns ok|mine|win|invalid|inactive."""
        if self.phase in {"won", "lost", "time_limit"}:
            return "inactive"
        if not self.in_bounds(row, col):
            return "invalid"
        if (row, col) in self.revealed or (row, col) in self.flagged:
            return "invalid"
        if not self.placed:
            self._place_mines(row, col)
        self.moves += 1
        if (row, col) in self.mine_cells:
            self.revealed.add((row, col))
            self.boom = (row, col)
            self.phase = "lost"
            return "mine"
        self._flood_reveal(row, col)
        if len(self.revealed) >= self.safe_total:
            self.phase = "won"
            return "win"
        self.phase = "playing"
        return "ok"

    def flag(self, row: int, col: int) -> str:
        """Toggle a flag. Returns flagged|unflagged|invalid|inactive."""
        if self.phase in {"won", "lost", "time_limit"}:
            return "inactive"
        if not self.in_bounds(row, col) or (row, col) in self.revealed:
            return "invalid"
        if (row, col) in self.flagged:
            self.flagged.discard((row, col))
            return "unflagged"
        self.flagged.add((row, col))
        return "flagged"

    def _place_mines(self, safe_row: int, safe_col: int) -> None:
        safe = {(safe_row, safe_col)}
        neighbors = set(self.neighbors(safe_row, safe_col))
        pool = [
            (row, col)
            for row in range(self.height)
            for col in range(self.width)
            if (row, col) not in safe and (row, col) not in neighbors
        ]
        if len(pool) < self.mines:
            # A dense board: only guarantee the clicked cell is safe.
            pool = [
                (row, col)
                for row in range(self.height)
                for col in range(self.width)
                if (row, col) != (safe_row, safe_col)
            ]
        self.mine_cells = set(self.rng.sample(pool, self.mines))
        self.placed = True
        self.phase = "playing"

    def _flood_reveal(self, row: int, col: int) -> None:
        stack = [(row, col)]
        while stack:
            current = stack.pop()
            if current in self.revealed or current in self.mine_cells or current in self.flagged:
                continue
            self.revealed.add(current)
            if self.adjacent_mines(*current) == 0:
                for neighbor in self.neighbors(*current):
                    if neighbor not in self.revealed:
                        stack.append(neighbor)

    # -- projections --------------------------------------------------------

    def projection(self) -> dict[str, object]:
        """Sanitized board for the browser. Mines are only shown once the game ends.

        Cells are a flat string of ``width * height`` tokens in row-major order
        (``?`` hidden, ``F`` flag, ``*`` mine, else the digit). A flat string keeps
        a 500x500 board small enough to stream.
        """
        show_mines = self.phase in {"lost", "won", "time_limit"}
        tokens: list[str] = []
        for row in range(self.height):
            for col in range(self.width):
                cell = (row, col)
                if cell in self.revealed:
                    if cell in self.mine_cells:
                        tokens.append(MINE)
                    else:
                        tokens.append(str(self.adjacent_mines(row, col)))
                elif cell in self.flagged:
                    tokens.append(FLAG)
                elif show_mines and cell in self.mine_cells:
                    tokens.append(MINE)
                else:
                    tokens.append(HIDDEN)
        return {
            "width": self.width,
            "height": self.height,
            "mines": self.mines,
            "phase": self.phase,
            "round": self.moves,
            "cells": "".join(tokens),
            "revealed_count": len(self.revealed),
            "safe_total": self.safe_total,
            "flags": len(self.flagged),
            "boom": list(self.boom) if self.boom else None,
            "outcome": {
                "status": self.phase,
                "moves": self.moves,
                "revealed": len(self.revealed),
                "safe_total": self.safe_total,
            },
        }
