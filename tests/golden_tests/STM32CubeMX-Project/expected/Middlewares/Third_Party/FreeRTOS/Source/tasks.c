#include <stdlib.h>
#include <string.h>

#define MPU_WRAPPERS_INCLUDED_FROM_API_FILE

/* FreeRTOS includes. */
#include "include/FreeRTOS.h"  // IWYU: keep
#include "include/task.h"
#include "timers.h"
#include "stack_macros.h"

#undef MPU_WRAPPERS_INCLUDED_FROM_API_FILE /*lint !e961 !e750 !e9021. */

#if ( configUSE_STATS_FORMATTING_FUNCTIONS == 1 )
	#include <stdio.h>
#endif /* configUSE_STATS_FORMATTING_FUNCTIONS == 1 ) */


#ifdef FREERTOS_MODULE_TEST
	#include "tasks_test_access_functions.h"
#endif


#if( configINCLUDE_FREERTOS_TASK_C_ADDITIONS_H == 1 )

	#include "freertos_tasks_c_additions.h"

#endif

