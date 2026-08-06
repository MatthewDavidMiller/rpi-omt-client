#define _POSIX_C_SOURCE 200809L
#include "deployer.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

static bool append(char **output, size_t *used, const char *data, size_t count,
                   omt_text_callback progress, void *context, char *error, size_t error_size) {
    char *grown;
    if (*used + count > OMT_DEPLOYER_OUTPUT_LIMIT) {
        if (error != NULL && error_size > 0U) (void)snprintf(error,error_size,"Child process output exceeded 4 MiB.");
        return false;
    }
    grown = realloc(*output, *used + count + 1U);
    if (grown == NULL) {
        if (error != NULL && error_size > 0U) (void)snprintf(error,error_size,"Unable to allocate process output.");
        return false;
    }
    *output = grown; memcpy(grown + *used, data, count); *used += count; grown[*used] = '\0';
    if (progress != NULL) progress(data, count, context);
    return true;
}

void omt_process_result_free(omt_process_result *result) {
    if (result != NULL) { free(result->output); result->output = NULL; result->exit_code = -1; }
}

#ifdef _WIN32
static wchar_t *utf16(const char *value) {
    const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value, -1, NULL, 0);
    wchar_t *result;
    if (size <= 0) return NULL;
    result = malloc((size_t)size * sizeof(*result));
    if (result != NULL) (void)MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,value,-1,result,size);
    return result;
}

static wchar_t *build_command(const char *const *arguments) {
    size_t units = 1U;
    wchar_t *result;
    for (size_t i=0U; arguments[i]!=NULL; ++i) {
        wchar_t *wide=utf16(arguments[i]); if (wide==NULL) return NULL;
        units += wcslen(wide)*2U+4U; free(wide);
    }
    result=calloc(units,sizeof(*result)); if(result==NULL)return NULL;
    for(size_t i=0U;arguments[i]!=NULL;++i){
        wchar_t *wide=utf16(arguments[i]); size_t slashes=0U;
        if(wide==NULL){free(result);return NULL;}
        if(i>0U)wcscat(result,L" ");
        wcscat(result,L"\"");
        for(const wchar_t *p=wide;*p!=L'\0';++p){
            if(*p==L'\\'){++slashes;continue;}
            if(*p==L'"'){for(size_t n=0U;n<slashes*2U+1U;++n)wcscat(result,L"\\");}
            else for(size_t n=0U;n<slashes;++n)wcscat(result,L"\\");
            {wchar_t one[2]={*p,L'\0'};wcscat(result,one);} slashes=0U;
        }
        for(size_t n=0U;n<slashes*2U;++n)wcscat(result,L"\\");
        wcscat(result,L"\""); free(wide);
    }
    return result;
}
#endif

bool omt_run_process(const char *const *arguments, const char *directory,
                     omt_text_callback progress, omt_stop_callback stop, void *context,
                     omt_process_result *result, char *error, size_t error_size) {
    size_t used = 0U;
    if (result == NULL || arguments == NULL || arguments[0] == NULL) return false;
    result->exit_code = -1; result->output = NULL;
#ifdef _WIN32
    SECURITY_ATTRIBUTES attributes={sizeof(attributes),NULL,TRUE};
    HANDLE read_handle=NULL,write_handle=NULL,job=NULL,input_handle=NULL;
    PROCESS_INFORMATION process={0}; STARTUPINFOW startup={0};
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits={0};
    wchar_t *command=NULL,*wide_directory=NULL;
    bool cancelled=false,ok=true;
    if(!CreatePipe(&read_handle,&write_handle,&attributes,0) ||
       !SetHandleInformation(read_handle,HANDLE_FLAG_INHERIT,0)) goto fail;
    job=CreateJobObjectW(NULL,NULL); limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if(job==NULL||!SetInformationJobObject(job,JobObjectExtendedLimitInformation,&limits,sizeof(limits)))goto fail;
    input_handle=CreateFileW(L"NUL",GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE,&attributes,OPEN_EXISTING,0,NULL);
    command=build_command(arguments); wide_directory=utf16(directory);
    if(input_handle==INVALID_HANDLE_VALUE||command==NULL||wide_directory==NULL)goto fail;
    startup.cb=sizeof(startup);startup.dwFlags=STARTF_USESTDHANDLES;
    startup.hStdInput=input_handle;startup.hStdOutput=write_handle;startup.hStdError=write_handle;
    if(!CreateProcessW(NULL,command,NULL,NULL,TRUE,CREATE_NO_WINDOW|CREATE_SUSPENDED,NULL,wide_directory,&startup,&process))goto fail;
    CloseHandle(write_handle);write_handle=NULL;CloseHandle(input_handle);input_handle=NULL;
    if(!AssignProcessToJobObject(job,process.hProcess)||ResumeThread(process.hThread)==(DWORD)-1)goto fail;
    for(;;){
        char buffer[16384];DWORD available=0,count=0;
        if(stop!=NULL&&stop(context)){cancelled=true;TerminateJobObject(job,ERROR_CANCELLED);}
        if(PeekNamedPipe(read_handle,NULL,0,NULL,&available,NULL)&&available>0U){
            const DWORD wanted=available>(DWORD)sizeof(buffer)?(DWORD)sizeof(buffer):available;
            if(ReadFile(read_handle,buffer,wanted,&count,NULL)&&count>0U)
                if(!append(&result->output,&used,buffer,(size_t)count,progress,context,error,error_size)){ok=false;break;}
        }
        if(WaitForSingleObject(process.hProcess,50)==WAIT_OBJECT_0){
            DWORD remaining=0;if(!PeekNamedPipe(read_handle,NULL,0,NULL,&remaining,NULL)||remaining==0U)break;
        }
    }
    if(!ok)TerminateJobObject(job,126);
    {DWORD code=1;GetExitCodeProcess(process.hProcess,&code);result->exit_code=(int)code;}
    if(cancelled){if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Operation cancelled.");ok=false;}
    free(command);free(wide_directory);CloseHandle(read_handle);CloseHandle(process.hThread);
    CloseHandle(process.hProcess);CloseHandle(job);return ok;
fail:
    if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Unable to start the child process safely.");
    if(process.hProcess!=NULL)TerminateProcess(process.hProcess,126);
    if(input_handle!=NULL&&input_handle!=INVALID_HANDLE_VALUE)CloseHandle(input_handle);
    if(write_handle!=NULL)CloseHandle(write_handle);
    if(read_handle!=NULL)CloseHandle(read_handle);
    if(process.hThread!=NULL)CloseHandle(process.hThread);
    if(process.hProcess!=NULL)CloseHandle(process.hProcess);
    if(job!=NULL)CloseHandle(job);
    free(command);free(wide_directory);return false;
#else
    int descriptors[2];
    pid_t child;
    int status=0;
    bool reaped=false,closed=false,ok=true;
    if (pipe(descriptors) != 0) goto fail;
    (void)fcntl(descriptors[0],F_SETFD,FD_CLOEXEC);(void)fcntl(descriptors[1],F_SETFD,FD_CLOEXEC);
    child=fork();
    if(child<0){close(descriptors[0]);close(descriptors[1]);goto fail;}
    if(child==0){
        (void)setpgid(0,0);close(descriptors[0]);
        if(chdir(directory)!=0||dup2(descriptors[1],STDOUT_FILENO)<0||dup2(descriptors[1],STDERR_FILENO)<0)_exit(126);
        close(descriptors[1]);execvp(arguments[0],(char *const *)(uintptr_t)arguments);_exit(errno==ENOENT?127:126);
    }
    close(descriptors[1]);(void)setpgid(child,child);
    {const int flags=fcntl(descriptors[0],F_GETFL,0);if(flags<0||fcntl(descriptors[0],F_SETFL,flags|O_NONBLOCK)<0){ok=false;}}
    while(ok&&(!reaped||!closed)){
        struct pollfd descriptor={descriptors[0],POLLIN,0};char buffer[16384];
        if(stop!=NULL&&stop(context)){if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Operation cancelled.");ok=false;break;}
        (void)poll(&descriptor,1,100);
        for(;;){
            const ssize_t count=read(descriptors[0],buffer,sizeof(buffer));
            if(count>0){if(!append(&result->output,&used,buffer,(size_t)count,progress,context,error,error_size)){ok=false;break;}}
            else if(count==0){closed=true;break;}
            else if(errno==EINTR)continue;
            else if(errno==EAGAIN||errno==EWOULDBLOCK)break;
            else{ok=false;break;}
        }
        if(!reaped&&waitpid(child,&status,WNOHANG)==child)reaped=true;
    }
    if(!ok&&!reaped){
        (void)kill(-child,SIGTERM);(void)kill(child,SIGTERM);
        /* Track the reap: signalling a pid or pgid that has already been reaped
           would target whatever the kernel has since recycled the id onto. */
        for(int i=0;i<10&&!reaped;++i){
            if(waitpid(child,&status,WNOHANG)==child){reaped=true;break;}
            (void)poll(NULL,0,50);
        }
        if(!reaped&&waitpid(child,&status,WNOHANG)==child)reaped=true;
        if(!reaped){(void)kill(-child,SIGKILL);(void)kill(child,SIGKILL);while(waitpid(child,&status,0)<0&&errno==EINTR){}}
    }
    close(descriptors[0]);
    result->exit_code=WIFEXITED(status)?WEXITSTATUS(status):128;
    return ok;
fail:
    if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Unable to create or fork child process.");
    return false;
#endif
}
