FROM nvidia/cuda:11.7.1-cudnn8-devel-ubuntu22.04

# Install Rust and build tools
RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Set CUDA environment variables
ENV CUDA_ROOT=/usr/local/cuda
ENV PATH=${CUDA_ROOT}/bin:${PATH}
ENV LD_LIBRARY_PATH=${CUDA_ROOT}/lib64:${LD_LIBRARY_PATH}
ENV CUDACXX=/usr/local/cuda/bin/nvcc

WORKDIR /app

# Copy only Cargo.toml files first to cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY ekko-location/Cargo.toml ekko-location/

# Create dummy main.rs to allow cargo build for dependency caching
RUN mkdir -p ekko-location/src && echo "fn main() {}" > ekko-location/src/main.rs

# Build dependencies (will cache unless Cargo.toml changes)
RUN CUDA_COMPUTE_CAP=75 cargo build --release -p ekko-location || true

# Remove dummy main.rs before actual build
RUN rm ekko-location/src/main.rs

# Now copy full source code
COPY . .

# Build the real application
RUN CUDA_COMPUTE_CAP=75 cargo build --release -p ekko-location
    
WORKDIR /app/ekko-location
CMD ["../target/release/ekko-location"]