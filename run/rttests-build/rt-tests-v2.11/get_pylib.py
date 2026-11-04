#!/usr/bin/python3
# SPDX-License-Identifier: GPL-2.0-or-later

# This file is used during the make process

import sysconfig

# Older versions of python don't have this scheme
# but produce the answer we expect with 'posix_prefix'
# Newer versions of python insert 'local' in the path unless we use rpm_prefix
SCHEME = 'rpm_prefix'

if not SCHEME in sysconfig.get_scheme_names():
    SCHEME = 'posix_prefix'

PYLIB = sysconfig.get_path('purelib', SCHEME)
print(PYLIB)
