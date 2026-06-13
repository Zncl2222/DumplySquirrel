FROM node:20-alpine AS builder

WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
ARG VITE_API_BASE_URL=/api
ENV VITE_API_BASE_URL=${VITE_API_BASE_URL}
RUN npm run build

FROM nginx:1.27-alpine

COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx/templates /etc/nginx/custom-templates
COPY nginx/docker-entrypoint.d /docker-entrypoint.d
RUN chmod +x /docker-entrypoint.d/99-dumply-render-config.sh

EXPOSE 80 443
