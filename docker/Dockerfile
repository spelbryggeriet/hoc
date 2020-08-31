FROM rustlang/rust:nightly AS builder
ARG APP_NAME=homepi-stream-manager

RUN curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

# Build dependencies.
WORKDIR /${APP_NAME}
RUN USER=root cargo init --bin ./src/backend \
    && USER=root cargo init --lib ./src/frontend
COPY ./Cargo.toml ./Cargo.toml
COPY ./src/backend/Cargo.* ./src/backend/
COPY ./src/frontend/Cargo.* ./src/frontend/
RUN cargo build --release -p ${APP_NAME}-backend \
    && cargo build --release -p ${APP_NAME}-frontend
RUN find src \( -name "*.rs" \) -exec rm {} \;

ADD . ./

WORKDIR /${APP_NAME}/target/release/deps
RUN rm `ls | grep -F "$(echo ${APP_NAME} | sed s/-/_/g)"`

# Build project.
WORKDIR /${APP_NAME}
RUN cargo build --release -p ${APP_NAME}-backend
RUN wasm-pack build \
    --release \
    --target web \
    --out-name package \
    --out-dir ../../static/pkg \
    ./src/frontend

FROM debian:buster-slim
ARG APP_PATH=/usr/src/app
ARG APP_NAME=homepi-stream-manager
ARG SERVER_NAME=${APP_NAME}-backend

RUN apt-get update \
    && apt-get install -y ca-certificates tzdata libpq-dev \
    && rm -rf /var/lib/apt/lists/*

EXPOSE 8000

ENV TZ=Etc/UTC \
    APP_USER=appuser

RUN groupadd ${APP_USER} \
    && useradd -g ${APP_USER} ${APP_USER} \
    && mkdir -p ${APP_PATH}

COPY --from=builder /${APP_NAME}/target/release/${SERVER_NAME} ${APP_PATH}/app
COPY --from=builder /${APP_NAME}/Rocket.toml ${APP_PATH}/Rocket.toml
COPY --from=builder /${APP_NAME}/static ${APP_PATH}/static

RUN chown -R ${APP_USER}:${APP_USER} ${APP_PATH}

USER ${APP_USER}
WORKDIR ${APP_PATH}

ENV HOMEPI_STATIC_DIR=${APP_PATH}/static \
    HOMEPI_DB_HOST=postgres \
    HOMEPI_DB_USER=postgresadmin \
    HOMEPI_DB_PASSWORD=admin123 \
    HOMEPI_DB_NAME=homepi-stream-manager
ENTRYPOINT [ "./app" ]
