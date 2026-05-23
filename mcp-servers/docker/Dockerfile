# CarpAI MCP Server - Docker (access host socket)
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-docker.txt ./
RUN pip install --no-cache-dir -r requirements-docker.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
