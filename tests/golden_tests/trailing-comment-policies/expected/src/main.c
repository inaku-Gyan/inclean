#include "mylib/internal/foo.h"  /* note A */
#include "mylib/private/bar.h"  // user note
#include "mylib/private/baz.h"  // note B
#include "mylib/helper/qux.h" /* please keep note C */
#include "mylib/legacy/old.h"  // note D

int main(void) {
    return mylib_foo() + mylib_bar() + mylib_baz() + mylib_qux() + mylib_old();
}
