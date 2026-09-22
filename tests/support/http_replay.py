from __future__ import annotations


class ReplayTransport:
    def __init__(self, responses: dict[str, bytes] | None = None, posts: dict[str, bytes] | None = None):
        self.responses = responses or {}
        self.posts = posts or {}
        self.get_calls: list[tuple[str, object]] = []
        self.post_calls: list[tuple[str, object, object]] = []

    async def get(self, url: str, *, headers=None) -> bytes:
        self.get_calls.append((url, headers))
        return self.responses[url]

    async def post(self, url: str, body=None, *, headers=None) -> bytes:
        self.post_calls.append((url, body, headers))
        return self.posts[url]


__all__ = ["ReplayTransport"]
