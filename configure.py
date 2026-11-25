"""Generate the single-owner configuration in the current checkout."""
import argparse
import os
import secrets
import shlex
from pathlib import Path
from urllib.parse import urlsplit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("origin", help="External HTTPS origin, e.g. https://glasses.example.com")
    args = parser.parse_args()
    origin = args.origin.rstrip("/")
    url = urlsplit(origin)
    if (url.scheme != "https" or not url.hostname or url.username is not None
            or url.password is not None or url.query or url.fragment or url.path
            or any(c.isspace() for c in origin)):
        parser.error("Enter an HTTPS origin without credentials, a path, query, or fragment.")
    data = Path("data").resolve()
    data.mkdir(mode=0o700, exist_ok=True)
    settings = {"DOT_ORIGIN": origin, "DOT_DB": str(data / "inbox.sqlite")}
    for key in ("DOT_OWNER_ID", "DOT_PAIR", "DOT_CLIENT_SECRET", "DOT_EVEN_TOKEN"):
        settings[key] = secrets.token_urlsafe(32)
    try:
        fd = os.open("service.env", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        parser.exit(1, "Existing service.env preserved. Edit it to change configuration.\n")
    with os.fdopen(fd, "w") as out:
        for key, value in settings.items():
            print(f"{key}={shlex.quote(value)}", file=out)
    print("Created service.env. Open it locally when pairing your devices.")


if __name__ == "__main__":
    main()
