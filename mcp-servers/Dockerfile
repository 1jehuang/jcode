# CarpAI MCP Server - Generic Dockerfile
# Build: docker build -t carpai/mcp-{name} .
# Run:   docker run -e API_KEY=xxx carpai/mcp-{name}

FROM python:3.12-slim

WORKDIR /app

# Install base dependencies
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Install server-specific dependencies if they exist
COPY requirements-*.txt ./
RUN for f in requirements-*.txt; do \
    if [ -f "$f" ] && [ "$f" != "requirements.txt" ]; then \
        pip install --no-cache-dir -r "$f"; \
    fi \
    done

# Copy server code
COPY src/server.py .

# Expose MCP port (default 8000 for SSE mode)
EXPOSE 8000

# Run the server
CMD ["python", "server.py"]
