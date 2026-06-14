"""Deliberately broken routes for diagnostic e2e tests (E17 §2.5)."""
from fastapi import APIRouter, Depends, Request

from app.deps import get_db

router = APIRouter(prefix="/broken", tags=["broken"])


# route/param-missing-arg: path declares {book_id} but handler has no such param
@router.get("/{book_id}")
def get_book_broken(title: str):
    return {"title": title}


# di/depends-called: get_db is called inside Depends()
@router.post("/db-called")
def create_with_called_dep(db=Depends(get_db())):
    return db


# route/duplicate: same method + path as get_book_broken above (param names differ)
@router.get("/{id}")
def get_book_duplicate(id: int, db=Depends(get_db)):
    return {"id": id}


# url/unknown-name: url_for references a route name that does not exist
@router.get("/url-for-bad")
def url_for_bad(request: Request):
    return {"url": request.url_for("no_such_route")}
