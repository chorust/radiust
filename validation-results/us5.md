# User Story 5 validation

Status: framework partial

Batch order, duplicate rejection, collect/raise partial results, bounded stream cancellation, shared cache leases, corruption eviction, startup repair, expiry/LRU GC, no-cache behavior, configuration precedence, report redaction and resource-limit tests pass locally.

The cache is now used by acquisition for refs with a known upstream revision and holds shared read leases until the cached `RawFrame` closes. An unwritable cache falls back to a cache-disabled operation. Real multi-process load, provider retries against live endpoints and long-running memory benchmarks remain unverified.
