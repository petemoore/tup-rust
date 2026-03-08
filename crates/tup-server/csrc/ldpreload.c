/*
 * ldpreload.c - LD_PRELOAD shared library for file access tracking.
 *
 * This library is injected into child processes via LD_PRELOAD to
 * intercept file system operations and record them to a depfile
 * for tup's dependency tracking.
 *
 * Based on the original tup ldpreload by Mike Shal, rewritten for
 * the tup-rust project.
 *
 * This file must be compiled as C (not Rust) because:
 * 1. It's injected into arbitrary processes via LD_PRELOAD
 * 2. It wraps libc functions using dlsym(RTLD_NEXT, ...)
 * 3. It needs to be a minimal shared library
 */

#define _GNU_SOURCE
#include "tup_depfile.h"

#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <limits.h>
#include <sys/stat.h>

/* Current working directory cache */
static char cwd[PATH_MAX];
static int cwdlen = -1;

/* Saved original function pointers */
static int (*s_open)(const char *, int, ...);
static int (*s_open64)(const char *, int, ...);
static FILE *(*s_fopen)(const char *, const char *);
static FILE *(*s_fopen64)(const char *, const char *);
static int (*s_rename)(const char *, const char *);
static int (*s_unlink)(const char *);
static int (*s_symlink)(const char *, const char *);
static int (*s_execve)(const char *, char *const[], char *const[]);
static int (*s_chdir)(const char *);
static int (*s_fchdir)(int);

/* Mutex for thread safety */
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static int depfd = -1;
static int errored = 0;

/* Forward declarations */
static void handle_file(const char *file, const char *file2, int at);
static int ignore_file(const char *file);
static int update_cwd(void);
static int is_full_path(const char *file);

#define WRAP(ptr, name) \
	if(!ptr) { \
		ptr = dlsym(RTLD_NEXT, name); \
		if(!ptr) { \
			fprintf(stderr, "tup.ldpreload: Unable to wrap '%s'\n", name); \
			exit(1); \
		} \
	}

/* Fork safety handlers */
static void prepare(void) { pthread_mutex_lock(&mutex); }
static void parent(void)  { pthread_mutex_unlock(&mutex); }
static void child(void)   { pthread_mutex_unlock(&mutex); }

/* Constructor — called when the library is loaded */
static void init_fd(void) __attribute__((constructor));
static void init_fd(void)
{
	const char *depfile;

	if(pthread_atfork(prepare, parent, child) != 0) {
		fprintf(stderr, "tup error: Unable to set pthread atfork handlers.\n");
		errored = 1;
		return;
	}

	depfile = getenv(TUP_DEPFILE);
	if(!depfile) {
		/* Not running under tup — silently skip */
		return;
	}

	WRAP(s_open, "open");
	if(depfd < 0) {
		depfd = s_open(depfile, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0666);
		if(depfd < 0) {
			perror(depfile);
			fprintf(stderr, "tup error: Unable to write dependencies.\n");
			errored = 1;
		}
	}
}

/* Write helper */
static int write_all(int fd, const void *data, int size)
{
	if(write(fd, data, size) != size) {
		perror("write");
		return -1;
	}
	return 0;
}

static int is_full_path(const char *file)
{
	return file[0] == '/';
}

/* Core event recording function */
static void handle_file_locked(const char *file, const char *file2, int at)
{
	struct access_event event;
	int len, len2;

	if(errored || depfd < 0)
		return;
	if(ignore_file(file))
		return;

	len = strlen(file) + 1;  /* include NUL */
	len2 = file2[0] ? (strlen(file2) + 1) : 0;

	/* Prepend CWD for relative paths */
	if(!is_full_path(file))
		len += cwdlen + 1;
	if(file2[0] && !is_full_path(file2))
		len2 += cwdlen + 1;

	event.at = at;
	event.len = len;
	event.len2 = len2;

	if(write_all(depfd, &event, sizeof(event)) < 0)
		goto err;

	/* Write path1 */
	if(!is_full_path(file)) {
		if(write_all(depfd, cwd, cwdlen) < 0) goto err;
		if(write_all(depfd, "/", 1) < 0) goto err;
	}
	if(write_all(depfd, file, strlen(file) + 1) < 0)
		goto err;

	/* Write path2 */
	if(len2 > 0) {
		if(file2[0] && !is_full_path(file2)) {
			if(write_all(depfd, cwd, cwdlen) < 0) goto err;
			if(write_all(depfd, "/", 1) < 0) goto err;
		}
		if(write_all(depfd, file2, strlen(file2) + 1) < 0)
			goto err;
	}
	return;

err:
	errored = 1;
}

static void handle_file(const char *file, const char *file2, int at)
{
	pthread_mutex_lock(&mutex);
	if(cwdlen < 0)
		update_cwd();
	handle_file_locked(file, file2, at);
	pthread_mutex_unlock(&mutex);
}

/* Wrapped system calls */

int open(const char *pathname, int flags, ...)
{
	int rc;
	mode_t mode = 0;
	WRAP(s_open, "open");
	if(flags & O_CREAT) {
		va_list ap;
		va_start(ap, flags);
		mode = va_arg(ap, int);
		va_end(ap);
	}
	rc = s_open(pathname, flags, mode);
	if(rc >= 0) {
		int at = (flags & (O_WRONLY|O_RDWR)) ? ACCESS_WRITE : ACCESS_READ;
		handle_file(pathname, "", at);
	} else {
		handle_file(pathname, "", ACCESS_READ);
	}
	return rc;
}

int open64(const char *pathname, int flags, ...)
{
	int rc;
	mode_t mode = 0;
	WRAP(s_open64, "open64");
	if(flags & O_CREAT) {
		va_list ap;
		va_start(ap, flags);
		mode = va_arg(ap, int);
		va_end(ap);
	}
	rc = s_open64(pathname, flags, mode);
	if(rc >= 0) {
		int at = (flags & (O_WRONLY|O_RDWR)) ? ACCESS_WRITE : ACCESS_READ;
		handle_file(pathname, "", at);
	} else {
		handle_file(pathname, "", ACCESS_READ);
	}
	return rc;
}

FILE *fopen(const char *path, const char *mode)
{
	FILE *rc;
	WRAP(s_fopen, "fopen");
	rc = s_fopen(path, mode);
	if(rc) {
		int at = (mode[0] == 'r' && !strchr(mode, '+')) ? ACCESS_READ : ACCESS_WRITE;
		handle_file(path, "", at);
	} else {
		handle_file(path, "", ACCESS_READ);
	}
	return rc;
}

FILE *fopen64(const char *path, const char *mode)
{
	FILE *rc;
	WRAP(s_fopen64, "fopen64");
	rc = s_fopen64(path, mode);
	if(rc) {
		int at = (mode[0] == 'r' && !strchr(mode, '+')) ? ACCESS_READ : ACCESS_WRITE;
		handle_file(path, "", at);
	} else {
		handle_file(path, "", ACCESS_READ);
	}
	return rc;
}

int rename(const char *old, const char *new)
{
	int rc;
	WRAP(s_rename, "rename");
	rc = s_rename(old, new);
	if(rc == 0)
		handle_file(old, new, ACCESS_RENAME);
	return rc;
}

int unlink(const char *pathname)
{
	int rc;
	WRAP(s_unlink, "unlink");
	rc = s_unlink(pathname);
	if(rc == 0)
		handle_file(pathname, "", ACCESS_UNLINK);
	return rc;
}

int symlink(const char *target, const char *linkpath)
{
	int rc;
	WRAP(s_symlink, "symlink");
	rc = s_symlink(target, linkpath);
	if(rc == 0)
		handle_file(linkpath, "", ACCESS_WRITE);
	return rc;
}

int execve(const char *filename, char *const argv[], char *const envp[])
{
	WRAP(s_execve, "execve");
	handle_file(filename, "", ACCESS_READ);
	return s_execve(filename, argv, envp);
}

int chdir(const char *path)
{
	int rc;
	WRAP(s_chdir, "chdir");
	rc = s_chdir(path);
	if(rc == 0) {
		pthread_mutex_lock(&mutex);
		update_cwd();
		pthread_mutex_unlock(&mutex);
	}
	return rc;
}

int fchdir(int fd)
{
	int rc;
	WRAP(s_fchdir, "fchdir");
	rc = s_fchdir(fd);
	if(rc == 0) {
		pthread_mutex_lock(&mutex);
		update_cwd();
		pthread_mutex_unlock(&mutex);
	}
	return rc;
}

/* Ignore system paths */
static int ignore_file(const char *file)
{
	if(strncmp(file, "/dev/", 5) == 0) return 1;
	if(strncmp(file, "/sys/", 5) == 0) return 1;
	if(strncmp(file, "/proc/", 6) == 0) return 1;
	if(strstr(file, ".ccache")) return 1;
	return 0;
}

static int update_cwd(void)
{
	if(getcwd(cwd, sizeof(cwd)) == NULL) {
		perror("getcwd");
		return -1;
	}
	cwdlen = strlen(cwd);
	return 0;
}
