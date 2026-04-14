// minimal chrome extension API types for zafu_sign
declare namespace chrome {
  namespace runtime {
    function sendMessage(extensionId: string, message: any, callback: (response: any) => void): void
    const lastError: { message?: string } | undefined
  }
}
