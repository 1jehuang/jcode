# CarpAI MCP Server - Slack
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt requirements-slack.txt ./
RUN pip install --no-cache-dir -r requirements-slack.txt
COPY src/server.py .
EXPOSE 8000
CMD ["python", "server.py"]
