from typing import Generator


def get_db() -> Generator:
    """Yield a fake DB session."""
    db = {"books": []}
    try:
        yield db
    finally:
        pass
