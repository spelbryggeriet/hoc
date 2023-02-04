FROM alpine:3.17.0

RUN echo http://dl-cdn.alpinelinux.org/alpine/edge/testing >> /etc/apk/repositories \
    && apk add kubectl helm \
    && helm repo add longhorn https://charts.longhorn.io

COPY ./helm /helm
ENV KUBECONFIG=/hoc/files/admin/kube/config
