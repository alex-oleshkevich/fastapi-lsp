"""Tests for codelens_fixture — imports TestClient directly so it has indicators."""
from fastapi.testclient import TestClient

from app import app

client = TestClient(app)


def test_list_items():
    response = client.get("/items")
    assert response.status_code == 200


def test_create_item():
    response = client.post("/items")
    assert response.status_code == 200
