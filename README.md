# Pumpkin Terrain Scanner

A high-performance, adaptive terrain surface height generation system for Minecraft-like voxel worlds, built for the Pumpkin project. Built on the pumpkin-mc crate.

## What This Is

This is a **terrain surface height scanner** for Minecraft-like voxel world generation. It figures out **where the ground surface is** in a 3D chunk of terrain - essentially scanning from the sky downward to find where solid ground begins at each X,Z coordinate.

## The Problem it Solves

When generating terrain, you need to know "what's the surface height at coordinate (5, 12)?" This seems simple, but:

- **Chunks are 3D volumes** (16x16x384 blocks typically)
- **Scanning everything is slow** - checking every single block from sky to bedrock takes forever
- **Terrain varies wildly** - flat plains vs tall mountains vs deep valleys
- **You need it fast** for scanning large areas

## Features

- 🚀 **High Performance**: Optimized memory access patterns and algorithmic improvements
- 🎯 **Adaptive Scanning**: Dynamically adjusts search ranges based on terrain characteristics
- 📊 **Smart Estimation**: Uses statistical methods to predict surface locations
- 🔄 **Early Exit Optimization**: Stops scanning early when sufficient data is collected
- 🌍 **Multi-Dimension Support**: Works with Overworld, Nether, and End dimensions
- 💾 **Memory Efficient**: Flat array structures for better cache performance
- 💾 **Block Storage in Postgresql**: Integrates with PostgreSQL for efficient, persistent storage of block and terrain data, enabling scalable world saving and retrieval.
