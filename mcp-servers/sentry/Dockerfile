# CarpAI MCP Server - Sentry
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-sentry.txt ./
RUN pip install --no-cache-dir -r requirements-sentry.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
