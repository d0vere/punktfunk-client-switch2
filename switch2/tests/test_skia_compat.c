/* Executable host tests of the exact Switch 2 portability shim. */
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <unistd.h>

extern void pf2_skia_compat_link_guard(void);
extern ssize_t pf2_skia_file_pread(int, void *, size_t, off_t);
extern void *pf2_skia_readonly_mmap(void *, size_t, int, int, int, off_t);
extern int pf2_skia_readonly_munmap(void *, size_t);

struct SharedRead { int fd; const char *expected; size_t len; };
static void *exercise_parallel_pread(void *arg) {
    struct SharedRead *in = arg;
    for (int i=0; i<1000; i++) {
        char result[64];
        assert(in->len <= sizeof(result));
        assert(pf2_skia_file_pread(in->fd,result,in->len,1) == (ssize_t)in->len);
        assert(memcmp(result,in->expected,in->len) == 0);
    }
    return NULL;
}

int main(void) {
    pf2_skia_compat_link_guard();
    FILE *f = tmpfile();
    assert(f != NULL);
    const char content[] = "punktfunk-skia-read-only-map";
    assert(fwrite(content, 1, sizeof(content), f) == sizeof(content));
    assert(fflush(f) == 0);
    int fd = fileno(f);
    assert(fd >= 0);
    assert(lseek(fd, 4, SEEK_SET) == 4);

    char bytes[8] = {0};
    assert(pf2_skia_file_pread(fd, bytes, 7, 1) == 7);
    assert(memcmp(bytes, content + 1, 7) == 0);
    assert(lseek(fd, 0, SEEK_CUR) == 4);

    void *mem = pf2_skia_readonly_mmap(NULL, sizeof(content), PROT_READ, MAP_PRIVATE, fd, 0);
    assert(mem != MAP_FAILED);
    assert(memcmp(mem, content, sizeof(content)) == 0);
    assert(lseek(fd, 0, SEEK_CUR) == 4);
    assert(pf2_skia_readonly_munmap(mem, sizeof(content)) == 0);
    // This explicitly tests only callers of our shim. An unrelated lseek
    // from another library remains outside the shim's mutex contract.
    struct SharedRead shared = {fd, content+1, sizeof(content)-1};
    pthread_t threads[6];
    for (size_t i=0; i<6; i++) {
        assert(pthread_create(&threads[i], NULL, exercise_parallel_pread, &shared) == 0);
    }
    for (size_t i=0; i<6; i++) assert(pthread_join(threads[i], NULL) == 0);
    assert(lseek(fd, 0, SEEK_CUR) == 4);
    errno = 0;
    assert(pf2_skia_readonly_mmap(NULL, 0, PROT_READ, MAP_PRIVATE, fd, 0) == MAP_FAILED);
    assert(errno == EINVAL);
    errno = 0;
    assert(pf2_skia_readonly_mmap(NULL, sizeof(content), PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0) == MAP_FAILED);
    assert(errno == ENOSYS);
    errno = 0;
    assert(pf2_skia_file_pread(fd, bytes, sizeof(bytes), -1) == -1);
    assert(errno == EINVAL);
    fclose(f);
    puts("PASS Skia/ICU newlib read-only mmap/pread/munmap compatibility");
    return 0;
}
