"""src/ layout fixture — module paths require source-root inference (E07 §3.4)."""
from fastapi import FastAPI

app = FastAPI()


@app.get("/ping")
def ping():
    return {"pong": True}
