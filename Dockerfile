# AI Continuity Bridge — Docker image for the compression-service
# (Python + FastAPI + LangChain LLM compression microservice)
FROM python:3.12-slim

WORKDIR /app

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

COPY compression-service/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY compression-service/ .

EXPOSE 8000

CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8000"]
 
