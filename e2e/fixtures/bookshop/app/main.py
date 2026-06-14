# 日本語 🎉  (non-ASCII comment — REQ-TST-03: position math after non-ASCII)
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates

from app.routers import books, broken_routes

app = FastAPI(title="Bookshop")

app.add_middleware(CORSMiddleware, allow_origins=["*"])

app.include_router(books.router, prefix="/api")
app.include_router(broken_routes.router)
app.mount("/static", StaticFiles(directory="static"), name="static")

templates = Jinja2Templates(directory="app/templates")
