"""Fixture for goto_definition / references tests — self-contained (dep defined in same file)."""
from fastapi import Depends, FastAPI

app = FastAPI()


def get_current_user():
    """A dep defined in the same FastAPI file so it appears in file_facts."""
    return {"user": "alice"}


@app.get("/profile")
def get_profile(user=Depends(get_current_user)):
    return user


@app.get("/settings")
def get_settings(user=Depends(get_current_user)):
    return {}
