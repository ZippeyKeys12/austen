int identity(int x)
/*@ requires true;
    ensures return == x;
@*/
{
    return x;
}
