from pydantic import BaseModel


class Book(BaseModel):
    id: int
    title: str
    author: str


class BookCreate(BaseModel):
    title: str
    author: str
