#include "mylib/internal/foo.h"  // pulled in for mylib_foo
#include "mylib/internal/bar.h"
#include <stdio.h>

int main(void) {
    return mylib_foo() + mylib_bar();
}
