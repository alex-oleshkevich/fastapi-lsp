"""Bookshop tests — used by F04 test-linking feature tests."""


def test_list_books(client):
    response = client.get("/api/books/")
    assert response.status_code == 200


def test_get_book(client):
    response = client.get("/api/books/0")
    assert response.status_code == 200


def test_create_book(client):
    response = client.post("/api/books/", json={"title": "Rust", "author": "Steve"})
    assert response.status_code == 201
