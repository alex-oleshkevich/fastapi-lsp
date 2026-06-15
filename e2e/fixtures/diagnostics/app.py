"""Fixture covering diagnostic codes not present in bookshop/broken_routes.py."""
import os
from fastapi import APIRouter, FastAPI, Request

app = FastAPI()

router_a = APIRouter(prefix="/a")
router_b = APIRouter(prefix="/b")
# router_unused is declared but never included in app → route/router-not-included
router_unused = APIRouter(prefix="/unused")


# env/undefined-key: references a key not in any .env file
@app.get("/env")
def read_env():
    val = os.environ["UNDEFINED_SECRET_KEY"]
    return {"val": val}


# route/duplicate-name: two routes share the same name (function name)
@router_a.get("/foo", name="shared_name")
def route_foo():
    return {}


@router_a.get("/bar", name="shared_name")
def route_bar():
    return {}


# Triggers route/duplicate-name: shared_name is actually used in url_for
@app.get("/dup-name-trigger")
def dup_name_trigger(request: Request):
    return {"url": str(request.url_for("shared_name"))}


# route/shadowed: /{id} declared before /featured makes /featured unreachable
@router_b.get("/{id}")
def get_by_id(id: str):
    return {"id": id}


@router_b.get("/featured")
def get_featured():
    return {"featured": True}


# url/param-mismatch: url_for called with wrong param name
@app.get("/url-mismatch")
def url_mismatch(request: Request):
    return {"url": str(request.url_for("get_by_id", wrong_param="x"))}


# route/arg-missing-param: path has {book_id} but handler uses 'book_idd' (edit distance 1)
@router_a.get("/item/{book_id}")
def handler_typo_arg(book_idd: int):
    return {"id": book_idd}


# model/unknown-response-model: UnknownModel is not defined/imported
@router_a.get("/model", response_model="UnknownModel")
def handler_unknown_model():
    return {}


app.include_router(router_a)
app.include_router(router_b)
# router_unused intentionally NOT included
