/*
 * Redirect the Miyoo ports' old App/<Port>/data paths into the game folder's
 * data directory. The ports hard-code that path for save states, while MAGI
 * installs the game (and its data) under Games/N64/<Game>.
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

static const char *mapped(const char *path, char *buf, size_t cap) {
    const char *app;
    const char *data;
    const char *root;
    const char *rel;

    if (!path) {
        return path;
    }
    app = strstr(path, "App/");
    data = strstr(path, "/data");
    if (!app || !data || data < app) {
        return path;
    }
    root = getenv("MAGI_N64_DATA");
    if (!root || !*root) {
        return path;
    }
    rel = data + 5;
    if (*rel == '/') {
        rel++;
    }
    if (*rel) {
        snprintf(buf, cap, "%s/%s", root, rel);
    } else {
        snprintf(buf, cap, "%s", root);
    }
    return buf;
}

#define REAL(name) dlsym(RTLD_NEXT, name)

int open(const char *path, int flags, ...) {
    static int (*real)(const char *, int, ...);
    char buf[PATH_MAX];
    mode_t mode = 0;
    if (!real) real = (int (*)(const char *, int, ...)) REAL("open");
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = (mode_t) va_arg(ap, int);
        va_end(ap);
    }
    return real(mapped(path, buf, sizeof buf), flags, mode);
}

int open64(const char *path, int flags, ...) {
    static int (*real)(const char *, int, ...);
    char buf[PATH_MAX];
    mode_t mode = 0;
    if (!real) real = (int (*)(const char *, int, ...)) REAL("open64");
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = (mode_t) va_arg(ap, int);
        va_end(ap);
    }
    return real(mapped(path, buf, sizeof buf), flags, mode);
}

int openat(int dirfd, const char *path, int flags, ...) {
    static int (*real)(int, const char *, int, ...);
    char buf[PATH_MAX];
    mode_t mode = 0;
    if (!real) real = (int (*)(int, const char *, int, ...)) REAL("openat");
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = (mode_t) va_arg(ap, int);
        va_end(ap);
    }
    return real(dirfd, mapped(path, buf, sizeof buf), flags, mode);
}

int openat64(int dirfd, const char *path, int flags, ...) {
    static int (*real)(int, const char *, int, ...);
    char buf[PATH_MAX];
    mode_t mode = 0;
    if (!real) real = (int (*)(int, const char *, int, ...)) REAL("openat64");
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = (mode_t) va_arg(ap, int);
        va_end(ap);
    }
    return real(dirfd, mapped(path, buf, sizeof buf), flags, mode);
}

FILE *fopen(const char *path, const char *mode) {
    static FILE *(*real)(const char *, const char *);
    char buf[PATH_MAX];
    if (!real) real = (FILE * (*)(const char *, const char *)) REAL("fopen");
    return real(mapped(path, buf, sizeof buf), mode);
}

FILE *fopen64(const char *path, const char *mode) {
    static FILE *(*real)(const char *, const char *);
    char buf[PATH_MAX];
    if (!real) real = (FILE * (*)(const char *, const char *)) REAL("fopen64");
    return real(mapped(path, buf, sizeof buf), mode);
}

int rename(const char *oldpath, const char *newpath) {
    static int (*real)(const char *, const char *);
    char a[PATH_MAX], b[PATH_MAX];
    if (!real) real = (int (*)(const char *, const char *)) REAL("rename");
    return real(mapped(oldpath, a, sizeof a), mapped(newpath, b, sizeof b));
}

int renameat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath) {
    static int (*real)(int, const char *, int, const char *);
    char a[PATH_MAX], b[PATH_MAX];
    if (!real) real = (int (*)(int, const char *, int, const char *)) REAL("renameat");
    return real(olddirfd, mapped(oldpath, a, sizeof a),
                 newdirfd, mapped(newpath, b, sizeof b));
}

int unlink(const char *path) {
    static int (*real)(const char *);
    char buf[PATH_MAX];
    if (!real) real = (int (*)(const char *)) REAL("unlink");
    return real(mapped(path, buf, sizeof buf));
}

int stat(const char *path, struct stat *st) {
    static int (*real)(const char *, struct stat *);
    char buf[PATH_MAX];
    if (!real) real = (int (*)(const char *, struct stat *)) REAL("stat");
    return real(mapped(path, buf, sizeof buf), st);
}

int lstat(const char *path, struct stat *st) {
    static int (*real)(const char *, struct stat *);
    char buf[PATH_MAX];
    if (!real) real = (int (*)(const char *, struct stat *)) REAL("lstat");
    return real(mapped(path, buf, sizeof buf), st);
}

int access(const char *path, int mode) {
    static int (*real)(const char *, int);
    char buf[PATH_MAX];
    if (!real) real = (int (*)(const char *, int)) REAL("access");
    return real(mapped(path, buf, sizeof buf), mode);
}
