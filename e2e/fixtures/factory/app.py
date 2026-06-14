"""App-factory pattern: app built inside create_app() (REQ-ROUTE-12)."""
from fastapi import APIRouter, FastAPI


def create_app(debug: bool = False) -> FastAPI:
    app = FastAPI(debug=debug)
    router = APIRouter(prefix="/items")

    @router.get("/")
    def list_items():
        return []

    @router.get("/{item_id}")
    def get_item(item_id: int):
        return {"id": item_id}

    app.include_router(router)
    return app


app = create_app()
