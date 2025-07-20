# HGecko

```
hgecko <path/to/asm/folder> <path/to/output/codes.gct>
```

A fast and simple alternative to Fizzi's [gecko](https://github.com/JLaferri/gecko/) assembler.

Used in [Training Mode - Community Edition](https://github.com/AlexanderHarrison/TrainingMode-CommunityEdition).

## Comparison with Gecko
**Pros:**
- ~10x faster.
- No temporary file issues on windows.
- Catches and reports undefined symbols.
- Returns a non-zero exit code on failure, making scripting easier.
- Simple usage - no JSON configuration.

**Cons:**
- Cannot control injection method.
- Cannot output to other formats (e.g. gecko code).
- Less tested.
