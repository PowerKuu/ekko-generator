# Coding Patterns to Avoid - Lessons Learned

## Variable Naming Anti-Patterns

### L DON'T: Use redundant qualifiers like "actual_", "real_", "true_", "final_"

**Bad:**
```rust
let actual_chunks_processed = batch_coords.len();
let actual_radius = config.radius.unwrap_or(default_radius);
```

**Good:**
```rust
let chunks_processed = batch_coords.len();
let radius = config.radius.unwrap_or(default_radius); 
```

**Why this is bad:**
- Implies other variables are somehow "fake" or incorrect
- Creates mental overhead - what makes this one "actual"?
- Often indicates a design problem where you have multiple variables representing the same concept

### L DON'T: Have calculation functions that don't match their usage

**Problem we encountered:**
```rust
// This calculates a SQUARE area (side²)
fn calculate_chunks_for_chunk_radius(radius: i32) -> usize {
    (2 * radius + 1) * (2 * radius + 1)  // 10201 chunks
}

// But this generates a CIRCULAR area (À*r²)  
fn generate_radius_coords(radius: i32) -> Vec<(i32, i32)> {
    // Only includes chunks where x² + z² d radius²
    // Returns 7845 chunks, not 10201!
}
```

**The fix:** Always ensure calculation functions match their actual usage patterns.

## Summary

**Variable names should be clear and direct. If you need qualifiers like "actual_", you probably have a design problem.**