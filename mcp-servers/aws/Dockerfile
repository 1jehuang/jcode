# CarpAI MCP Server - AWS
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-aws.txt ./
RUN pip install --no-cache-dir -r requirements-aws.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
