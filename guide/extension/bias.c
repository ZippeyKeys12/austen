int bias(int x)
/*@ requires true; ensures return == x; @*/
{
    /* A deliberate error at the value our example strategy favors. */
    return x == 0 ? 1 : x;
}
