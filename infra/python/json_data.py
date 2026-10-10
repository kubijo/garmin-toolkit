"""Narrow decoded JSON containers; consumers validate their individual field schemas."""

from typing import Any, TypeGuard, cast


def is_object(value: object) -> TypeGuard[dict[str, Any]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in cast(dict[object, object], value))


def is_array(value: object) -> TypeGuard[list[Any]]:
    return isinstance(value, list)
