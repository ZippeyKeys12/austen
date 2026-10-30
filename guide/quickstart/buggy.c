int identity(int x)
/*@ requires true;
    ensures return == x;
@*/
{
    /* Deliberate bug: odd inputs lose their value. */
    return x % 2 == 0 ? x : 0;
}
