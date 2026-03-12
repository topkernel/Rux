/*
 * Test: mkdir() / rmdir()
 * Test directory operations
 */

#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <dirent.h>

int main(void)
{
    const char *dir_path = "/tmp/test_mini_ltp_dir";
    DIR *dir;

    /* Create directory */
    if (mkdir(dir_path, 0755) < 0) return 1;

    /* Verify directory exists */
    dir = opendir(dir_path);
    if (dir == NULL) {
        rmdir(dir_path);
        return 2;
    }

    /* Close directory */
    if (closedir(dir) < 0) {
        rmdir(dir_path);
        return 3;
    }

    /* Remove directory */
    if (rmdir(dir_path) < 0) return 4;

    /* Verify directory is removed */
    dir = opendir(dir_path);
    if (dir != NULL) {
        closedir(dir);
        return 5;
    }

    return 0;
}
