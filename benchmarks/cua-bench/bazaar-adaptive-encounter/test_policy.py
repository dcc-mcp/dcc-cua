import json
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).parent
sys.path.insert(0, str(ROOT))

from policy import best_safe_candidate  # noqa: E402


class AdaptiveEncounterPolicyTests(unittest.TestCase):
    def test_variants_change_the_best_safe_choice(self):
        cases = json.loads((ROOT / "cases.json").read_text(encoding="utf-8"))
        selected = {
            case["id"]: best_safe_candidate(case)["id"]
            for case in cases
        }
        self.assertEqual(
            selected,
            {
                "day7-measured-recovery": "center",
                "healthy-opportunity-window": "right",
                "low-health-survival": "left",
                "preserve-active-relationship": "left",
            },
        )

    def test_active_effect_is_a_hard_constraint(self):
        cases = json.loads((ROOT / "cases.json").read_text(encoding="utf-8"))
        scenario = next(
            case for case in cases if case["id"] == "preserve-active-relationship"
        )
        self.assertGreater(
            next(card for card in scenario["candidates"] if card["id"] == "center")[
                "currentBuildUtility"
            ],
            next(card for card in scenario["candidates"] if card["id"] == "left")[
                "currentBuildUtility"
            ],
        )
        self.assertEqual(best_safe_candidate(scenario)["id"], "left")


if __name__ == "__main__":
    unittest.main()
