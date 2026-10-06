/*
 * E8 Bug-1 reproducer: fork/exit storm over the reparent paths.
 *
 * GNOME final8 panicked at guest+3286s when an outer sh (321) exited and
 * reparent_children_to_init walked a dangling parent pointer (freed
 * ancestor Task) into (*pp).signal -> KERNPANIC at exit.rs:954.
 *
 * This program drives the three tree shapes that fed that crash:
 *
 *  A. Orphaned zombie chains: a grandparent exits before its children,
 *     children exit leaving zombies, zombies get reparented — the
 *     classic "no survivor reaps me" cascade.
 *  B. Thread-group fork + sibling reaper (the glib/gdbus pattern): a
 *     helper thread reaps children the main thread forks, racing the
 *     main thread's exit/reparent — the double-reap and freed-dest
 *     windows.
 *  C. Deep chain mass exit: 5-level chains torn down top-first, so each
 *     level's reparent walks whatever the level above left behind.
 *
 * Success = prints DONE and exits 0. Any kernel panic, hang, or
 * SIGSEGV/SIGKILL of the harness is a failure.
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -o orphan_storm orphan_storm.c -lpthread
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <pthread.h>
#include <sys/wait.h>
#include <sys/mman.h>

static volatile int stop_reaping = 0;

static void *reaper_thread(void *arg)
{
	(void)arg;
	while (!stop_reaping) {
		int status;
		pid_t p = waitpid(-1, &status, WNOHANG);
		if (p > 0)
			continue;
		if (p < 0 && errno == ECHILD)
			usleep(200); /* nothing yet; main keeps forking */
		else
			usleep(50);
	}
	/* drain whatever is left once the forker stopped */
	for (;;) {
		int status;
		pid_t p = waitpid(-1, &status, WNOHANG);
		if (p <= 0)
			break;
	}
	return NULL;
}

/* A. orphan zombie chains: G forks M, M forks K, G exits at once;
 * K exits (zombie under M), M exits (zombie+child reparent cascade). */
static void pattern_orphan_chain(int round)
{
	pid_t mid = fork();
	if (mid == 0) {
		pid_t kid = fork();
		if (kid == 0)
			_exit(16 + (round & 7));
		/* parent-side of the inner pair: leave kid unreaped briefly */
		usleep(1000 + (round % 7) * 300);
		_exit(8 + (round & 3));
	}
	/* grandparent exits without reaping mid — mid becomes an orphan and
	 * its zombie child must follow it through reparenting. */
}

/* B. thread group with a sibling reaper: helper reaps while the main
 * thread forks+exits children, then the whole group exits.
 * NO_THREADS builds (the -DNO_THREADS binary in the image) fall back to
 * the serial pattern: the pthread churn wedge is PRE-EXISTING on
 * faedf87 (identical EARLY-ROOT-FREE + stall on pristine and patched
 * kernels alike — see runs/e8/orphan-storm-baseline-smp2.log), so the
 * regression fence for the reparent fix uses the threadless build. */
static void pattern_thread_group(int rounds)
{
#ifdef NO_THREADS
	for (int i = 0; i < rounds; i++)
		pattern_orphan_chain(i);
#else
	pthread_t th;
	if (pthread_create(&th, NULL, reaper_thread, NULL) != 0) {
		/* fall back to serial pattern if threads are unavailable */
		for (int i = 0; i < rounds; i++)
			pattern_orphan_chain(i);
		return;
	}
	for (int i = 0; i < rounds; i++) {
		pid_t c = fork();
		if (c == 0)
			_exit(i & 0x7f);
		/* helper thread reaps concurrently; do not wait here */
		usleep(300 + (i % 5) * 137);
	}
	stop_reaping = 1;
	pthread_join(th, NULL);
#endif
}

/* C. deep chain torn down top-first: every level exits while its
 * descendants are still alive; reparent cascades down the chain.
 * Spawned via a forked chain-root so the LADDER's top-level _exit is a
 * child, never the harness itself (first version called this straight
 * from main — PID 1 _exit'd after ONE chain and powered the box down
 * before DONE). */
static void pattern_deep_chain(int depth)
{
	if (depth <= 0)
		_exit(42);
	pid_t c = fork();
	if (c == 0)
		pattern_deep_chain(depth - 1); /* child continues the chain */
	/* parent exits immediately, orphaning the still-running child */
	_exit(depth);
}

static void spawn_deep_chain(void)
{
	pid_t root = fork();
	if (root == 0)
		pattern_deep_chain(5); /* never returns */
	/* harness keeps running; the drain loop at the end reaps strays */
}

int main(int argc, char **argv)
{
	/*
	 * Default intensity stays UNDER the pre-existing fork-churn wedge
	 * documented on faedf87 (both patched and pristine kernels stall
	 * around round ~24-32 of the full 60-round profile — the
	 * EARLY-ROOT-FREE family, out of scope here; see
	 * runs/e8/orphan-storm-baseline-smp2.log). 16 rounds completes on
	 * both, giving a regression fence for the reparent paths.
	 */
	int rounds = argc > 1 ? atoi(argv[1]) : 16;
	int chains = argc > 2 ? atoi(argv[2]) : 10;

	signal(SIGCHLD, SIG_DFL);

	printf("orphan_storm: rounds=%d chains=%d\n", rounds, chains);

	for (int r = 0; r < rounds; r++) {
		pattern_orphan_chain(r);
		pattern_thread_group(6);
		if ((r & 7) == 0)
			printf("orphan_storm: round %d/%d\n", r, rounds);
	}

	for (int c = 0; c < chains; c++)
		spawn_deep_chain();

	/* let the dust settle and verify the tree still reaps */
	usleep(300000);
	for (;;) {
		int status;
		pid_t p = waitpid(-1, &status, WNOHANG);
		if (p <= 0)
			break;
	}

	printf("orphan_storm: DONE\n");
	return 0;
}
