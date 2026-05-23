# CarpAI MCP Server - Redis
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-redis.txt ./
RUN pip install --no-cache-dir -r requirements-redis.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
