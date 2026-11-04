# SPDX-License-Identifier: GPL-2.0-or-later
# Use CentOS Stream 9 as base image
FROM centos:stream9

# Install required packages for RT-Tests
RUN dnf -y update && \
    dnf install -y \
        gcc \
        make \
        numactl-devel \
        util-linux \
        git && \
    git clone https://git.kernel.org/pub/scm/utils/rt-tests/rt-tests.git /opt/rt-tests && \
    cd /opt/rt-tests && \
    make && \
    make install && \
    dnf remove -y \
        git \
        make && \
    dnf clean all


# Set the working directory to the RT-Tests directory
WORKDIR /opt/rt-tests

# Set the entrypoint to a shell
ENTRYPOINT ["/bin/bash"]
