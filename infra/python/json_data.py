"""Narrow decoded JSON containers; consumers validate their individual field schemas."""

from typing import Any, TypeGuard, cast


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    """Reject ambiguous duplicate fields when decoding integrity metadata."""
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'Duplicate JSON key: {key}')
        result[key] = value
    return result


def is_object(value: object) -> TypeGuard[dict[str, Any]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in cast(dict[object, object], value))


def is_array(value: object) -> TypeGuard[list[Any]]:
    return isinstance(value, list)
