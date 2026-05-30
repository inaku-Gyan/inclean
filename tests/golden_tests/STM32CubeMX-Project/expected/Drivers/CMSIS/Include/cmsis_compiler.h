#ifndef __CMSIS_COMPILER_H
#define __CMSIS_COMPILER_H

#include <stdint.h>

/*
 * Arm Compiler 4/5
 */
#if   defined ( __CC_ARM )
  #include "cmsis_armcc.h"  // IWYU: keep

/*
 * GNU Compiler
 */
#elif defined ( __GNUC__ )
  #include "cmsis_gcc.h"  // IWYU: keep


/*
 * TI Arm Compiler
 */
#elif defined ( __TI_ARM__ )
  #include <cmsis_ccs.h>  // IWYU: keep

/*
 * COSMIC Compiler
 */
#elif defined ( __CSMC__ )
   #include <cmsis_csm.h>  // IWYU: keep

#else
  #error Unknown compiler.
#endif


#endif /* __CMSIS_COMPILER_H */

