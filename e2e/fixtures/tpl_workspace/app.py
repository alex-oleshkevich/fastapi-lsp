from fastapi import FastAPI, Request
from fastapi.responses import HTMLResponse
from fastapi.templating import Jinja2Templates

app = FastAPI()
templates = Jinja2Templates(directory="templates")


@app.get("/books", response_class=HTMLResponse)
def list_books(request: Request):
    return templates.TemplateResponse(request, "books.html")


@app.get("/admin", response_class=HTMLResponse)
def admin_view(request: Request):
    return templates.TemplateResponse(request, "admin/dashboard.html")


@app.get("/wrong", response_class=HTMLResponse)
def wrong_template(request: Request):
    return templates.TemplateResponse(request, "book.html")
