# Garmin's March 2026 Linux Manager links WebKitGTK 4.0/libsoup 2.4.
# Jammy supplies their actual ABI; do not substitute 4.1 symlinks.
FROM docker.io/library/ubuntu:22.04@sha256:8a2321a92b438238f93b4346fff61bc4259c99fdd4d9b52ee86f153fee0c344f
COPY sources.list /etc/apt/sources.list
COPY ca-bundle.crt /etc/ssl/certs/ca-certificates.crt
RUN apt-get -o Acquire::Check-Valid-Until=false -o APT::Update::Error-Mode=any update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
        ca-certificates fonts-dejavu-core libcurl4 libexpat1 libfontconfig1 \
        libfreetype6 libgtk-3-0 libjpeg8 libpng16-16 libsecret-1-0 libsm6 \
        libstdc++6 libudev1 libusb-1.0-0 libwebkit2gtk-4.0-37 libx11-6 libxext6 libxkbcommon0 \
        libxxf86vm1 zlib1g \
    && rm -rf /var/lib/apt/lists/*
