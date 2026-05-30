#ifndef __CMSIS_COMPILER_H
#define __CMSIS_COMPILER_H

#include <stdint.h>

/*
 * Arm Compiler 4/5
 */
#if   defined ( __CC_ARM )
  #include "cmsis_armcc.h"  // IWYU: export

/*
 * GNU Compiler
 */
#elif defined ( __GNUC__ )
  #include "cmsis_gcc.h"  // IWYU: export


/*
 * TI Arm Compiler
 */
#elif defined ( __TI_ARM__ )
  #include <cmsis_ccs.h>

/*
 * COSMIC Compiler
 */
#elif defined ( __CSMC__ )
   #include <cmsis_csm.h>

#else
  #error Unknown compiler.
#endif


#endif /* __CMSIS_COMPILER_H */

