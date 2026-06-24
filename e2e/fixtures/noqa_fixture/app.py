"""Fixture for # noqa suppression tests."""
from fastapi import FastAPI

app = FastAPI()


# route/param-missing-arg: path declares {book_id} but handler has no such param.
# The bare # noqa on the decorator line suppresses the diagnostic.
@app.get("/{book_id}")  # noqa
def get_book(title: str):
    return {"title": title}


# This route also has a missing param but NO # noqa — diagnostic must fire.
@app.get("/{item_id}")
def get_item(title: str):
    return {"title": title}
