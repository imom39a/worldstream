"""Unit tests for the pure Minesweeper game logic."""

from __future__ import annotations

import unittest

from examples.minesweeper.game import (
    MINE,
    MinesweeperError,
    MinesweeperGame,
    cell_label,
    parse_cell_label,
)


class MinesweeperGameTests(unittest.TestCase):
    def make(self, width: int = 5, height: int = 5, mines: int = 1, seed: int = 7) -> MinesweeperGame:
        return MinesweeperGame(width, height, mines, seed)

    def test_configuration_is_bounded(self) -> None:
        for width, height, mines in ((1, 5, 1), (5, 1, 1), (5, 5, 0), (5, 5, 25)):
            with self.assertRaises(MinesweeperError):
                MinesweeperGame(width, height, mines, 1)

    def test_first_reveal_is_always_safe(self) -> None:
        for seed in range(25):
            game = self.make(seed=seed)
            self.assertIn(game.reveal(2, 2), {"ok", "win"})
            self.assertNotIn((2, 2), game.mine_cells)
            self.assertNotEqual(game.phase, "lost")

    def test_same_seed_places_the_same_mines(self) -> None:
        first, second = self.make(seed=99), self.make(seed=99)
        first.reveal(0, 0)
        second.reveal(0, 0)
        self.assertEqual(first.mine_cells, second.mine_cells)

    def test_revealing_a_mine_loses(self) -> None:
        game = self.make()
        game.mine_cells = {(0, 0)}
        game.placed = True
        game.phase = "playing"
        self.assertEqual(game.reveal(0, 0), "mine")
        self.assertEqual(game.phase, "lost")
        self.assertEqual(game.boom, (0, 0))

    def test_flood_reveal_can_clear_the_board(self) -> None:
        game = self.make(mines=1, seed=3)
        game.mine_cells = {(0, 0)}
        game.placed = True
        game.phase = "playing"
        self.assertEqual(game.reveal(4, 4), "win")
        self.assertEqual(game.phase, "won")
        self.assertEqual(len(game.revealed), game.safe_total)

    def test_invalid_and_repeat_moves_are_rejected(self) -> None:
        game = self.make()
        self.assertEqual(game.reveal(2, 2), "ok")
        self.assertEqual(game.reveal(2, 2), "invalid")
        self.assertEqual(game.reveal(-1, 0), "invalid")
        self.assertEqual(game.flag(2, 2), "invalid")

    def test_flags_toggle_and_block_reveals(self) -> None:
        game = self.make()
        self.assertEqual(game.flag(1, 1), "flagged")
        self.assertEqual(game.reveal(1, 1), "invalid")
        self.assertEqual(game.flag(1, 1), "unflagged")
        self.assertEqual(game.reveal(1, 1), "ok")

    def test_candidates_are_hidden_and_ordered_by_frontier(self) -> None:
        game = self.make(mines=1, seed=11)
        game.reveal(2, 2)
        labels = [candidate["cell"] for candidate in game.candidates()]
        self.assertNotIn(cell_label(2, 2), labels)
        frontier = [candidate for candidate in game.candidates() if candidate["frontier"]]
        self.assertTrue(frontier)
        self.assertEqual(labels[0], frontier[0]["cell"])

    def test_candidates_respect_the_limit(self) -> None:
        game = self.make(width=10, height=10, mines=5, seed=1)
        self.assertEqual(len(game.candidates(limit=4)), 4)

    def test_agent_state_and_criteria_share_the_candidate_set(self) -> None:
        game = self.make(seed=5)
        game.reveal(0, 0)
        state = game.agent_state(round_number=1)
        criteria = game.choice_criteria()
        candidate_labels = [candidate["cell"] for candidate in state["candidates"]]
        self.assertEqual(list(criteria), candidate_labels)
        self.assertEqual(state["board"]["width"], 5)
        self.assertIn("grid", state["board"])
        self.assertIn("deductions", state)

    def _single_mine_corner(self) -> MinesweeperGame:
        """A 3x3 board where (0,0) is a mine and the last row is provably safe."""
        game = self.make(width=3, height=3, mines=1, seed=1)
        game.mine_cells = {(0, 0)}
        game.placed = True
        game.phase = "playing"
        game.revealed = {(0, 1), (0, 2), (1, 0), (1, 1), (1, 2)}
        return game

    def test_deductions_prove_safe_and_mine_cells(self) -> None:
        safe, mine = self._single_mine_corner().deduce()
        self.assertEqual(mine, {(0, 0)})
        self.assertEqual(safe, {(2, 0), (2, 1), (2, 2)})

    def test_offered_candidates_exclude_proven_mines(self) -> None:
        offered = self._single_mine_corner().offered_candidates()
        labels = [candidate["cell"] for candidate in offered]
        self.assertNotIn("r1c1", labels)
        self.assertEqual(set(labels), {"r3c1", "r3c2", "r3c3"})
        self.assertTrue(all(candidate["provably_safe"] for candidate in offered))
        self.assertTrue(all(0.0 <= candidate["mine_risk"] <= 1.0 for candidate in offered))

    def test_offered_candidates_fall_back_to_guesses_with_risk(self) -> None:
        game = self.make(seed=9)
        game.reveal(0, 0)
        offered = game.offered_candidates()
        self.assertTrue(offered)
        self.assertTrue(all(0.0 <= candidate["mine_risk"] <= 1.0 for candidate in offered))
        risks = [candidate["mine_risk"] for candidate in offered]
        self.assertEqual(risks, sorted(risks))

    def test_small_board_state_includes_grid(self) -> None:
        game = self.make(seed=5)
        game.reveal(0, 0)
        state = game.agent_state(round_number=1)
        self.assertIn("grid", state["board"])

    def test_large_board_state_is_bounded_and_grid_free(self) -> None:
        game = MinesweeperGame(120, 120, 99, 1)
        game.reveal(60, 60)
        state = game.agent_state(round_number=2, limit=20)
        self.assertNotIn("grid", state["board"])
        self.assertLessEqual(len(state["candidates"]), 20)
        self.assertEqual(
            list(game.choice_criteria(20)),
            [candidate["cell"] for candidate in state["candidates"]],
        )
        self.assertEqual(len(game.projection()["cells"]), 120 * 120)

    def test_offered_candidates_respect_limit_on_large_board(self) -> None:
        game = MinesweeperGame(200, 200, 99, 3)
        game.reveal(100, 100)
        self.assertLessEqual(len(game.offered_candidates(12)), 12)

    def test_many_mines_on_a_large_board(self) -> None:
        game = MinesweeperGame(200, 200, 9000, 4)
        self.assertIn(game.reveal(100, 100), {"ok", "win"})
        self.assertEqual(len(game.mine_cells), 9000)
        self.assertEqual(len(game.projection()["cells"]), 200 * 200)

    def test_projection_hides_mines_until_the_game_ends(self) -> None:
        game = self.make()
        game.mine_cells = {(0, 0)}
        game.placed = True
        game.phase = "playing"
        game.reveal(0, 1)
        self.assertEqual(game.phase, "playing")
        hidden = game.projection()
        self.assertIsInstance(hidden["cells"], str)
        self.assertEqual(len(hidden["cells"]), game.width * game.height)
        self.assertNotIn(MINE, hidden["cells"])
        game.reveal(0, 0)
        shown = game.projection()
        self.assertEqual(shown["cells"][0], MINE)


class CellLabelTests(unittest.TestCase):
    def test_round_trip(self) -> None:
        self.assertEqual(cell_label(0, 0), "r1c1")
        self.assertEqual(parse_cell_label("r3c5"), (2, 4))
        self.assertIsNone(parse_cell_label("3,5"))
        self.assertIsNone(parse_cell_label("r0c1"))
        self.assertIsNone(parse_cell_label(None))


if __name__ == "__main__":
    unittest.main()
