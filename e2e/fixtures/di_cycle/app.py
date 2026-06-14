"""Fixture for di/cycle diagnostic — mutual dependency between dep_b and dep_a."""
from fastapi import Depends, FastAPI

app = FastAPI()


# dep_b is defined first so dep_a can reference it by identifier.
# The LSP parses statically: dep_b depends on dep_a (defined below),
# dep_a depends on dep_b → cycle.
def dep_b(a=Depends(dep_a)):  # noqa: F821 — intentional forward ref for cycle detection
    return a


def dep_a(b=Depends(dep_b)):
    return b


@app.get("/cycle")
def handler(x=Depends(dep_a)):
    return {}
