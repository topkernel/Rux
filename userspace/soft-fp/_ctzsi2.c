/* Pin-clean replacement for Ubuntu libgcc's _ctzsi2.o member; see
 * _ctzdi2.c. */
int __ctzdi2(unsigned long x);
int __ctzsi2(unsigned int x)
{
    return __ctzdi2(x);
}
