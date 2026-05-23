# CarpAI MCP Server - GitHub
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-github.txt ./
RUN pip install --no-cache-dir -r requirements-github.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
