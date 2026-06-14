"""Fixture for di/override-unused diagnostic.

app.dependency_overrides[nonexistent_dep] = fake_fn  # noqa: F821 — intentional undefined dep
The dep name 'nonexistent_dep' is not a known dep_def in the workspace,
so di/override-unused fires.
"""
from fastapi import FastAPI

app = FastAPI()


def fake_fn():
    return "fake"


# di/override-unused: nonexistent_dep is not a dep_def in any workspace file
app.dependency_overrides[nonexistent_dep] = fake_fn  # noqa: F821 — intentional undefined dep
