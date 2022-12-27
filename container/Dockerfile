FROM alpine:3.17.0

RUN echo http://dl-cdn.alpinelinux.org/alpine/edge/testing >> /etc/apk/repositories
RUN apk add kubectl helm

COPY ./helm /helm
ENV KUBECONFIG=/hoc/files/admin/kube/config
