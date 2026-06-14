"""FastAPI app for code_lens e2e tests."""
from fastapi import FastAPI

app = FastAPI()


@app.get("/items")
def list_items():
    return []


@app.post("/items")
def create_item():
    return {}
