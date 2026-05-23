# CarpAI MCP Server - Datadog
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-datadog.txt ./
RUN pip install --no-cache-dir -r requirements-datadog.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
