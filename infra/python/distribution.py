"""Nearest-rank summaries shared by browser and desktop profiling."""

import math
from dataclasses import dataclass
from typing import Self


@dataclass(frozen=True)
class Distribution:
    count: int
    mean: float
    p50: float
    p95: float
    p99: float
    maximum: float

    @classmethod
    def from_values(cls, values: list[float]) -> Self | None:
        if not values:
            return None
        ordered = sorted(values)
        p50, p95, p99 = (ordered[math.ceil(len(ordered) * fraction) - 1] for fraction in (0.5, 0.95, 0.99))
        return cls(len(ordered), sum(ordered) / len(ordered), p50, p95, p99, ordered[-1])
