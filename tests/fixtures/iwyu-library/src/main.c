#include "foo.h"
#include "bar.h"  // user note
#include "baz.h"
#include "qux.h" /* please keep */
#include "old.h"  // legacy comment to be overwritten

int main(void) {
    return mylib_foo() + mylib_bar() + mylib_baz() + mylib_qux() + mylib_old();
}
