# CarpAI MCP Server - Kubernetes
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-kubernetes.txt ./
RUN pip install --no-cache-dir -r requirements-kubernetes.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
