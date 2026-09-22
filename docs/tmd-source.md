# Thailand TMD GIF source

The `th` adapter reads the UTC observation time printed in each GIF frame's
footer. The GIF container and HTTP response do not expose that time as metadata.
The Khon Kaen animation uses the last frame's time as `FrameRef.valid_time` and
retains all per-frame times in reference metadata. The adapter checks the
15-minute sequence and rejects unreadable text, an unexpected image layout, or
an invalid sequence instead of substituting the retrieval time.

Footer recognition requires the system **Tesseract OCR** executable. On macOS,
install it with `brew install tesseract`; on Debian or Ubuntu, install
`tesseract-ocr`. The Python package does not install the system binary. Without
it, TMD discovery fails with a clear error. The offline GitHub Actions workflow
installs Tesseract before running the source tests.

Query-driven SDK/CLI downloads keep discovery and acquisition in one operation
context, so they write the exact GIF whose footer established the reference
time. If a caller runs `discover` and later acquires the reference in a separate
operation, the adapter checks the downloaded SHA-256 against the reference and
fails if the mutable endpoint has changed; rediscover to get the new frame.

This OCR step binds frame identity to the provider's printed timestamp. It does
not interpret radar colors or establish pixel geometry. Scientific decoding
therefore remains unavailable until the source palette and native grid are
verified.

The retained GIFs and the current public endpoints show April 2023 observations
even though they were retrieved in September 2026. `--latest` reports those
observation times; a `max_age` constraint filters them as stale. See
[`migration/sources/th.json`](../migration/sources/th.json) and
[`test_th.py`](../tests/sources/test_th.py) for the recorded evidence.
