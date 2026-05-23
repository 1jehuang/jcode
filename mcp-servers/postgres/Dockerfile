# CarpAI MCP Server - PostgreSQL
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-postgres.txt ./
RUN pip install --no-cache-dir -r requirements-postgres.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
