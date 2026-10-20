/*
 * Copyright (c) 2024 Huawei Device Co., Ltd.
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#include "appspawn_server.h"

#undef _GNU_SOURCE
#define _GNU_SOURCE
#include <sched.h>
#include <signal.h>
#include <time.h>
#include <stdio.h>

#include "appspawn_trace.h"
#include "appspawn_utils.h"
#ifndef OHOS_LITE
#include "appspawn_manager.h"
#ifndef APPSPAWN_HELPER
#include "ffrt_inner.h"
#endif
#endif

#define MAX_FORK_TIME (30 * 1000)   // 30ms

static void NotifyResToParent(struct AppSpawnContent *content, AppSpawnClient *client, int result)
{
    StartAppspawnTrace("NotifyResToParent");
    APPSPAWN_LOGI("NotifyResToParent: %{public}d", result);
    APPSPAWN_CHECK(content != NULL && content->notifyResToParent != NULL, FinishAppspawnTrace();
        return, "invalid param");
    content->notifyResToParent(content, client, result);
    FinishAppspawnTrace();
}

void ProcessExit(int code)
{
    APPSPAWN_DUMPI("ExitCode:%{public}d", code);
#ifdef OHOS_LITE
    _exit(0x7f); // 0x7f user exit
#else
    quick_exit(0);
#endif
}

#ifdef APPSPAWN_HELPER
__attribute__((visibility("default")))
_Noreturn
void exit(int code)
{
    char *checkExit = getenv(APPSPAWN_CHECK_EXIT);
    if (checkExit && atoi(checkExit) == getpid()) {
        APPSPAWN_LOGF("Unexpected call: exit(%{public}d)", code);
        abort();
    }
    // hook `exit` to `ProcessExit` to ensure app exit in a clean way
    ProcessExit(code);
    // should not come here
    abort();
}
#endif

int AppSpawnChild(AppSpawnContent *content, AppSpawnClient *client)
{
    APPSPAWN_CHECK(content != NULL && client != NULL, return -1, "Invalid arg for appspawn child");
    APPSPAWN_DUMPI("AppSpawnChild id %{public}u flags:0x%{public}x", client->id, client->flags);
    StartAppspawnTrace("AppSpawnExecuteClearEnvHook");
    int ret = AppSpawnExecuteClearEnvHook(content, client);
    FinishAppspawnTrace();
    APPSPAWN_CHECK_ONLY_EXPER(ret == 0,
        NotifyResToParent(content, client, ret);
        AppSpawnEnvClear(content, client);
        return 0);

    if (client->flags & APP_COLD_START) {
        // cold start fail, to start normal
        if (content->coldStartApp != NULL && content->coldStartApp(content, client) == 0) {
            return 0;
        }
        APPSPAWN_LOGW("AppSpawnChild cold start fail %{public}u", client->id);
    }
    StartAppspawnTrace("AppSpawnExecuteSpawningHook");
    ret = AppSpawnExecuteSpawningHook(content, client);
    FinishAppspawnTrace();
    APPSPAWN_CHECK_ONLY_EXPER(ret == 0,
        NotifyResToParent(content, client, ret);
        AppSpawnEnvClear(content, client);
        return 0);
    StartAppspawnTrace("AppSpawnExecutePreReplyHook");
    ret = AppSpawnExecutePreReplyHook(content, client);
    FinishAppspawnTrace();
    APPSPAWN_CHECK_ONLY_EXPER(ret == 0,
        NotifyResToParent(content, client, ret);
        AppSpawnEnvClear(content, client);
        return 0);

    bool isAppspawn = (content->mode == MODE_FOR_APP_SPAWN || content->mode == MODE_FOR_APP_COLD_RUN);
    APPSPAWN_CHECK_ONLY_EXPER(isAppspawn,
        NotifyResToParent(content, client, 0));

    StartAppspawnTrace("AppSpawnExecutePostReplyHook");
    (void)AppSpawnExecutePostReplyHook(content, client);
    FinishAppspawnTrace();

    if (content->runChildProcessor != NULL) {
        ret = content->runChildProcessor(content, client);
    }
    if (ret != 0) {
        APPSPAWN_CHECK_ONLY_EXPER(!isAppspawn,
            NotifyResToParent(content, client, ret));
        AppSpawnEnvClear(content, client);
    }
    return 0;
}

static int CloneAppSpawn(void *arg)
{
    APPSPAWN_CHECK(arg != NULL, return -1, "Invalid content for appspawn");
    APPSPAWN_LOGI("CloneNwebSpawn done.");
#ifndef APPSPAWN_HELPER
    ffrt_child_init();
#endif
    AppSpawnForkArg *forkArg = (AppSpawnForkArg *)arg;
    ProcessExit(AppSpawnChild(forkArg->content, forkArg->client));
    return 0;
}

#ifndef OHOS_LITE
static void NwebSpawnCloneChildProcess(AppSpawnContent *content, AppSpawnClient *client, pid_t *pid)
{
    AppSpawnForkArg arg;
    arg.client = client;
    arg.content = content;
#ifndef APPSPAWN_TEST
    AppSpawningCtx *property = (AppSpawningCtx *)client;
    uint32_t len = 0;
    char *processType = (char *)(GetAppSpawnMsgExtInfo(property->message, MSG_EXT_NAME_PROCESS_TYPE, &len));
    APPSPAWN_CHECK(processType != NULL, return, "Invalid processType data");

    if (strcmp(processType, "gpu") == 0) {
        *pid = clone(CloneAppSpawn, NULL, CLONE_NEWNET | SIGCHLD, (void *)&arg);
    } else {
        *pid = clone(CloneAppSpawn, NULL, content->sandboxNsFlags | SIGCHLD, (void *)&arg);
    }
#else
    *pid = clone(CloneAppSpawn, NULL, content->sandboxNsFlags | SIGCHLD, (void *)&arg);
#endif
}
#endif

static void AppSpawnForkChildProcess(AppSpawnContent *content, AppSpawnClient *client, pid_t *pid)
{
    struct timespec forkStart = {0};
#ifndef OHOS_LITE
    enum fdsan_error_level errorLevel = fdsan_get_error_level();
#endif
    clock_gettime(CLOCK_MONOTONIC, &forkStart);
    StartAppspawnTrace("AppspawnFork");
    *pid = fork();
    if (*pid == 0) {
        struct timespec forkEnd = {0};
        clock_gettime(CLOCK_MONOTONIC, &forkEnd);
        uint64_t diff = DiffTime(&forkStart, &forkEnd);
        APPSPAWN_CHECK_ONLY_LOGW(diff < MAX_FORK_TIME, "fork time %{public}" PRId64 " us", diff);
#ifndef OHOS_LITE
        // Inherit the error level of the original process
        (void)fdsan_set_error_level(errorLevel);
#endif
        ProcessExit(AppSpawnChild(content, client));
    } else {
        FinishAppspawnTrace();
    }
}

int AppSpawnProcessMsg(AppSpawnContent *content, AppSpawnClient *client, pid_t *childPid)
{
    APPSPAWN_CHECK(content != NULL, return -1, "Invalid content for appspawn");
    APPSPAWN_CHECK(client != NULL && childPid != NULL, return -1, "Invalid client for appspawn");
    APPSPAWN_LOGI("AppSpawnProcessMsg id: %{public}d mode: %{public}d sandboxNsFlags: 0x%{public}x",
        client->id, content->mode, content->sandboxNsFlags);

    pid_t pid = 0;
#ifndef OHOS_LITE
    if (content->mode == MODE_FOR_NWEB_SPAWN) {
        NwebSpawnCloneChildProcess(content, client, &pid);
    } else {
#else
    {
#endif
        AppSpawnForkChildProcess(content, client, &pid);
    }
    APPSPAWN_CHECK(pid >= 0, return APPSPAWN_FORK_FAIL, "fork child process error: %{public}d", errno);
    *childPid = pid;
    return 0;
}
