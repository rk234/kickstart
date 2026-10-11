"""Seeds the backend with test data. Kickstart starts this once the backend port is open."""

import json
from urllib.request import Request, urlopen

BACKEND = "http://localhost:8000"
ITEMS = [
    {"name": "apple", "price": 1.25},
    {"name": "bread", "price": 3.50},
    {"name": "coffee", "price": 9.99},
]

for item in ITEMS:
    req = Request(
        f"{BACKEND}/items",
        data=json.dumps(item).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urlopen(req) as resp:
        print(f"seeded {item['name']} ({resp.status})")

print(f"seeded {len(ITEMS)} items")
