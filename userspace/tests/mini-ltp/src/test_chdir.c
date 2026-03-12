/*
 * Test: chdir() / getcwd()
 * Test directory switching
 */

#include <unistd.h>
#include <string.h>

int main(void)
{
    char cwd[256];
    char new_cwd[256];

    /* Get current directory */
    if (getcwd(cwd, sizeof(cwd)) == NULL) return 1;

    /* Switch to /tmp */
    if (chdir("/tmp") < 0) return 2;

    /* Verify directory has changed */
    if (getcwd(new_cwd, sizeof(new_cwd)) == NULL) return 3;

    if (strcmp(new_cwd, "/tmp") != 0) return 4;

    /* Switch back to original directory */
    if (chdir(cwd) < 0) return 5;

    return 0;
}
