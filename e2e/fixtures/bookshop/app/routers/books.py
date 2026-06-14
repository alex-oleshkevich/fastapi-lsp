from typing import Annotated

from fastapi import APIRouter, Depends

from app.deps import get_db
from app.models import Book, BookCreate

router = APIRouter(prefix="/books", tags=["books"])

DbDep = Annotated[dict, Depends(get_db)]


@router.get("/", response_model=list[Book])
def list_books(db: DbDep):
    return db["books"]


@router.get("/{book_id}", response_model=Book)
def get_book(book_id: int, db: DbDep):
    return db["books"][book_id]


@router.post("/", response_model=Book, status_code=201)
def create_book(book: BookCreate, db: DbDep):
    new_book = Book(id=len(db["books"]), **book.model_dump())
    db["books"].append(new_book)
    return new_book
