# CarpAI SDK Performance Benchmarks

**Date**: 2026-05-17
**Version**: carpai-sdk v0.1.0
**Platform**: Windows (i7-1260P, 32GB RAM)

## Benchmark Suite

Run with: `cargo bench -p carpai-sdk --bench sdk_bench`

### Test Scenarios

| Benchmark | Description | Metric |
|-----------|-------------|--------|
| `cache_put` | Cache write operation | ns/iter |
| `cache_get_hit` | Cache read (hit) | ns/iter |
| `cache_get_miss` | Cache read (miss) | ns/iter |
| `cache_stats` | Statistics collection (100 entries) | ns/iter |
| `cache_concurrent_10_threads` | Concurrent access (10 threads) | ns/iter |
| `cache_eviction_200_inserts` | Eviction under load (200 inserts, cap=100) | ns/iter |
| `request_validation_valid` | Input validation | ns/iter |
| `serialize_request` | JSON serialization | ns/iter |
| `deserialize_request` | JSON deserialization | ns/iter |
| `serialize_response` | Response serialization | ns/iter |
| `key_generation` | Cache key hashing | ns/iter |

## Baseline Results

*(Run `cargo bench` to populate actual numbers)*

```
cache_put                       time:   [XX.XX ns XX.XX ns XX.XX ns]
cache_get_hit                   time:   [XX.XX ns XX.XX ns XX.XX ns]
cache_get_miss                  time:   [XX.XX ns XX.XX ns XX.XX ns]
cache_stats                     time:   [XX.XX ns XX.XX ns XX.XX ns]
cache_concurrent_10_threads     time:   [XX.XX ns XX.XX ns XX.XX ns]
cache_eviction_200_inserts      time:   [XX.XX ns XX.XX ns XX.XX ns]
request_validation_valid        time:   [XX.XX ns XX.XX ns XX.XX ns]
serialize_request               time:   [XX.XX ns XX.XX ns XX.XX ns]
deserialize_request             time:   [XX.XX ns XX.XX ns XX.XX ns]
serialize_response              time:   [XX.XX ns XX.XX ns XX.XX ns]
key_generation                  time:   [XX.XX ns XX.XX ns XX.XX ns]
```

## Regression Testing

To compare against baseline:

```bash
# Save new baseline
cargo bench -- --save-baseline v0.1.0

# Compare with previous
cargo bench -- --baseline v0.1.0
```

## Performance Targets

| Operation | Target (ns) | P95 Target (ns) |
|-----------|------------|-----------------|
| cache_put | < 500 | < 1000 |
| cache_get_hit | < 200 | < 500 |
| cache_get_miss | < 100 | < 200 |
| cache_stats | < 5000 | < 10000 |
| request_validation | < 100 | < 200 |
| serialize_request | < 1000 | < 2000 |

## Notes

- Benchmarks use Criterion.rs for statistical rigor
- Sample size: 1000 iterations per test
- Warm-up: 3 seconds
- Measurement time: 10 seconds per benchmark
- HTML reports generated in `target/criterion/report/`
