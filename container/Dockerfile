FROM alpine:3.17.0

RUN echo http://dl-cdn.alpinelinux.org/alpine/edge/testing >> /etc/apk/repositories \
    && apk add kubectl helm curl python3 \
    && helm repo add longhorn https://charts.longhorn.io \
    && mkdir -p /usr/local/lib/python \
    && curl -sfL https://github.com/longhorn/longhorn-tests/raw/v1.4.0/manager/integration/tests/longhorn.py > /usr/local/lib/python/longhorn.py \
    && python3 -m ensurepip --upgrade \
    && pip3 install --upgrade pip \
    && pip3 install six requests

ENV PYTHONPATH=/usr/local/lib/python

COPY ./helm /helm
ENV KUBECONFIG=/hoc/files/admin/kube/config
