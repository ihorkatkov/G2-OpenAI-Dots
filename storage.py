import os
import sqlite3
from contextlib import contextmanager


@contextmanager
def database():
    db = sqlite3.connect(os.environ["DOT_DB"], timeout=10)
    db.row_factory = sqlite3.Row
    try:
        with db:
            yield db
    finally:
        db.close()


def init():
    with database() as db:
        db.executescript("""
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS grants (
          digest TEXT PRIMARY KEY, kind TEXT NOT NULL, expires REAL NOT NULL,
          challenge TEXT, redirect TEXT
        );
        CREATE TABLE IF NOT EXISTS subscriptions (
          id TEXT PRIMARY KEY, url TEXT NOT NULL, secret TEXT NOT NULL, expires REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS questions (
          id TEXT PRIMARY KEY, text TEXT NOT NULL, created REAL NOT NULL, answer TEXT
        );
        CREATE TABLE IF NOT EXISTS deferred_replies (
          question TEXT PRIMARY KEY REFERENCES questions(id)
        );
        CREATE TABLE IF NOT EXISTS deliveries (
          question TEXT NOT NULL, subscription TEXT NOT NULL, due REAL NOT NULL,
          attempts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(question, subscription)
        );
        """)
